//! Shared JKA player visual presentation for snapshot, followed, and local players.
//!
//! The data flow mirrors OpenJK's multiplayer cgame path: `ET_PLAYER` resolves
//! `clientInfo_t` from `CS_PLAYERS`, registers `model.glm` + skin, runs the
//! humanoid animation state, then submits Ghoul2 surfaces through the selected
//! CPU/worker/GPU skinning backend. Snapshot ownership stays in `cgame.rs` so a
//! network, demo, and local/offline sources use this exact same presentation code.

use crate::{
    asset_jobs::{self, AssetPriority, AssetRegistry, AssetSource, AssetState, Requested},
    cgame::{suppressed_during_intermission, ClientGameState, ForcedPlayerModels, Ghoul2ServerCommand, PresentationEvent, PresentedEntity, ET_BODY, ET_GENERAL, ET_NPC, ET_PLAYER, GT_SIEGE},
    materials::{self, TextureData, Textures},
    renderer::{
        DynamicModelAlphaMode, DynamicModelSurface, DynamicWireframeClass, DynamicModelVertex, FxGpuSpriteInstance,
        FxGpuSprites, Ghoul2GpuBone,
        Ghoul2GpuSkinning, Ghoul2GpuVertex,
    },
    scene,
    ui::Ghoul2SkinningMode,
};
use jka_assets::{
    animation::{animation_index, is_death_animation, load_humanoid_animations, AnimationSet},
    ghoul2::{
        model_bolt_matrix, multiply_3x4, openjk_default_gla, parse_gla, parse_glm,
        skin_glm_surface, smooth_ghoul2_pose, GlaAnimation, Ghoul2Animator, Ghoul2SkinnedSurface, GlmModel,
        GlmSurface, Matrix3x4, BONE_ANIM_OVERRIDE_FREEZE, BONE_ANIM_OVERRIDE_LOOP, OPENJK_DEFAULT_GLA_NAME,
    },
    pk3::AssetSearchPath,
    siege::find_siege_class_visual,
    saber::{
        load_saber_animation_scales, load_saber_definitions, SaberAnimationScales, SaberDefinition,
        SaberDefinitions,
    },
    shader::{AlphaGen, Bulge, RgbGen, Shader, Stage, TcGen, TcMod, Wave, WaveFunc},
    skin::load_skin,
    vehicle::{load_vehicle_definitions, VehicleDefinition, VehicleDefinitions},
};
use super::cloth::{ClothCapsule, ClothConfig, ClothMotion, ClothOutput, ClothSurfaceFrame, ClothSystem};
use super::footsteps::{self, FootstepImpact, FootstepStages};
use super::jiggle::{JiggleProfile, JiggleSystem};
use super::player_animation::PlayerAnimationState;

/// The viewer's model animation, as fed to `CG_PlayerAnimation` and as played.
/// `*_input` are the raw `legsAnim`/`torsoAnim` (toggle bit included).
#[derive(Debug, Clone, Copy, Default)]
pub struct ViewerAnimDebug {
    pub legs_input: i32,
    pub torso_input: i32,
    pub legs_anim: i32,
    pub legs_frame: i32,
    pub legs_old_frame: i32,
    pub legs_backlerp: f32,
    pub torso_anim: i32,
    pub torso_frame: i32,
    pub torso_old_frame: i32,
    pub torso_backlerp: f32,
    /// `current_time` the pose was last evaluated at.
    pub pose_time: i32,
    /// Where and how the model was placed: `entity.origin` / `entity.angles`.
    pub origin: [f32; 3],
    pub angles: [f32; 3],
    /// `*l_leg_foot` / `*r_leg_foot` in model space after posing; a pose pop shows here.
    pub feet: [[f32; 3]; 2],
    /// Low 32 bits of a hash of the model/skin key, and whether the real model was ready.
    pub model_hash: u32,
    pub model_settled: bool,
    /// Posed bolt positions in model space (right hand, left hand, head, chest),
    /// from the same `pose` that is skinned and drawn. Zero when a bolt is missing.
    pub bones: [[f32; 3]; 4],
    /// The transform the model is actually drawn with (`ghoul2_render_transform`):
    /// origin and the facing yaw of its forward axis, in degrees.
    pub render_origin: [f32; 3],
    pub render_yaw: f32,
    /// Set by `present_player_entity` after the call: its `current_time`, whether
    /// it was asked to submit geometry, and how many surfaces it returned.
    pub call_time: i32,
    pub submit_geometry: bool,
    pub surfaces: u32,
    /// World-space anchors from the same pose and transform used for drawing.
    pub head_world: Option<[f32; 3]>,
    pub hilt_world: [Option<[f32; 3]>; 2],
    pub hilt_submitted: [bool; 2],
    pub blade_world: [[Option<[f32; 3]>; 8]; 2],
    pub blade_core_world: [[Option<[f32; 3]>; 8]; 2],
    pub blade_length: [[f32; 8]; 2],
    pub blade_submitted: [[bool; 8]; 2],
}
use super::ragdoll::{
    DetachedLimbGeneration, DetachedLimbSpawn, PhysicsMapMesh, RagdollConfig, RagdollMode,
    RagdollWorld,
};
use super::saber_throw::{blade_angles, SaberThrowState};
use jka_movement::{
    bg_g2_player_angles, CollisionWorld, PlayerAngleEntity, PlayerAngleState, TraceQuery, TraceWorld,
    BONE_ANGLES_POSTMULT,
};
use jka_protocol::entity_event::EntityEvent;
use glam::Vec3;
use rayon::prelude::*;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};

const EF_TELEPORT_BIT: i32 = 1 << 3;
const WP_SABER: i32 = 3;
const WP_BRYAR_PISTOL: i32 = 4;
// OpenJK bg_public.h: server-authored detached Ghoul2 limb entities.
const G2_MODEL_PART: i32 = 50;
const G2_MODELPART_HEAD: i32 = 10;
const G2_MODELPART_WAIST: i32 = 11;
const G2_MODELPART_LARM: i32 = 12;
const G2_MODELPART_RARM: i32 = 13;
const G2_MODELPART_RHAND: i32 = 14;
const G2_MODELPART_LLEG: i32 = 15;
const G2_MODELPART_RLEG: i32 = 16;
const EF_NODRAW: i32 = 1 << 8;
const EF_DEAD: i32 = 1 << 1;
const EF_RAG: i32 = 1 << 6;
const EF2_SHIP_DEATH: i32 = 1 << 7;

// OpenJK MP CG_PlayerShadow / bg_public.h / teams.h values. Blob shadows are
// deliberately a CGame presentation feature, not part of the CSM/RT paths.
const PW_CLOAKED: i32 = 11;
const CLASS_REMOTE: i32 = 39;
const CLASS_SEEKER: i32 = 41;
// Stock OpenJK MP CG_PlayerShadow uses SHADOW_DISTANCE 128; jaPRO, TaystJK and
// EternalJK raise it to 512, so the blob stays visible far below a jumping player.
const BLOB_SHADOW_DISTANCE: f32 = 512.0;
const BLOB_SHADOW_RADIUS: f32 = 24.0;
const BLOB_SHADOW_DROID_RADIUS: f32 = 8.0;
const BLOB_SHADOW_MINS: [f32; 3] = [-15.0, -15.0, 0.0];
const BLOB_SHADOW_MAXS: [f32; 3] = [15.0, 15.0, 2.0];
const MASK_PLAYERSOLID: i32 = 0x0000_0001 | 0x0000_0010 | 0x0000_0100 | 0x0000_1000;

const MAX_GLM_BYTES: usize = 64 * 1024 * 1024;
const MAX_GLA_BYTES: usize = 128 * 1024 * 1024;
const MAX_SKIN_BYTES: usize = 4 * 1024 * 1024;
const MAX_JIGGLE_BYTES: usize = 64 * 1024;


/// Renderer-view data used by the Ghoul2 presenter for the same whole-model
/// sphere cull and projected-radius LOD decision that OpenJK performs in
/// `R_AddGhoulSurfaces`/`G2_ComputeLOD`.
#[derive(Clone, Copy, Debug)]
pub struct Ghoul2PresentationView {
    position: Vec3,
    forward: Vec3,
    right: Vec3,
    up: Vec3,
    tan_half_fov_y: f32,
    aspect: f32,
    /// Effective worldspawn distanceCull (JKA units); <= 0 means unlimited.
    distance_cull: f32,
}

impl Ghoul2PresentationView {
    pub fn new(
        position: [f32; 3],
        forward: [f32; 3],
        fov_y: f32,
        aspect: f32,
        distance_cull: f32,
    ) -> Self {
        let forward = Vec3::from_array(forward).normalize_or_zero();
        let mut right = forward.cross(Vec3::Y).normalize_or_zero();
        if right.length_squared() < 1.0e-8 {
            right = Vec3::X;
        }
        let up = right.cross(forward).normalize_or_zero();
        Self {
            position: Vec3::from_array(position),
            forward,
            right,
            up,
            tan_half_fov_y: (fov_y * 0.5).tan().max(1.0e-6),
            aspect: aspect.max(1.0e-6),
            distance_cull,
        }
    }

    pub fn with_pose(self, position: [f32; 3], forward: [f32; 3]) -> Self {
        let forward = Vec3::from_array(forward).normalize_or_zero();
        let mut right = forward.cross(Vec3::Y).normalize_or_zero();
        if right.length_squared() < 1.0e-8 {
            right = Vec3::X;
        }
        let up = right.cross(forward).normalize_or_zero();
        Self {
            position: Vec3::from_array(position),
            forward,
            right,
            up,
            ..self
        }
    }

    pub(crate) fn sphere_outside(self, jka_origin: [f32; 3], radius: f32) -> bool {
        let center = Vec3::from_array(scene::render_position(jka_origin));
        let delta = center - self.position;
        let depth = delta.dot(self.forward);
        let radius = radius.max(0.0);

        // OpenJK's R_GCullModel only tests the four side frustum planes, not
        // zNear/zFar. Keep that behavior here: objects behind the camera are
        // rejected by the side planes just as they are by R_CullPointAndRadius.
        let x = delta.dot(self.right);
        let y = delta.dot(self.up);
        let tan_x = self.tan_half_fov_y * self.aspect;
        let side_x = depth * tan_x - x.abs();
        let side_y = depth * self.tan_half_fov_y - y.abs();
        let radius_x = radius * (1.0 + tan_x * tan_x).sqrt();
        let radius_y = radius * (1.0 + self.tan_half_fov_y * self.tan_half_fov_y).sqrt();
        side_x < -radius_x || side_y < -radius_y
    }

    pub(crate) fn distance_cull(self) -> f32 {
        self.distance_cull
    }

    pub(crate) fn aspect(self) -> f32 {
        self.aspect
    }

    pub(crate) fn fov_x_degrees(self) -> f32 {
        (2.0 * (self.tan_half_fov_y * self.aspect).atan()).to_degrees()
    }

    /// Camera origin converted back to JKA's native Z-up coordinates. Auxiliary
    /// CGame refEntities such as the invulnerability half-shield orient
    /// themselves from `cg.refdef.vieworg`, not from the player's viewangles.
    pub(crate) fn jka_position(self) -> [f32; 3] {
        scene::jka_position(self.position.to_array())
    }

    /// Straight-line distance from the view to a JKA-space point.
    pub(crate) fn distance_to(self, jka_origin: [f32; 3]) -> f32 {
        Vec3::from_array(scene::render_position(jka_origin)).distance(self.position)
    }

    fn projected_radius(self, jka_origin: [f32; 3], radius: f32) -> f32 {
        let center = Vec3::from_array(scene::render_position(jka_origin));
        let depth = (center - self.position).dot(self.forward);
        // ProjectRadius returns zero for a model at/behind the view plane; the
        // caller treats that as LOD0 (view weapon / near-plane intersection).
        if depth <= 0.0 {
            0.0
        } else {
            radius.abs() / (depth * self.tan_half_fov_y)
        }
    }
}

fn ghoul2_entity_radius(entity: &PresentedEntity) -> f32 {
    entity
        .state
        .field_i32("g2radius")
        .filter(|radius| *radius != 0)
        .map(|radius| radius as f32)
        .unwrap_or(64.0)
}

fn ghoul2_model_scale(entity: &PresentedEntity) -> f32 {
    let scale = entity.state.field_i32("iModelScale").unwrap_or(0);
    if scale == 0 {
        1.0
    } else {
        (scale as f32 / 100.0).abs().max(1.0e-6)
    }
}

/// TaystJK/OpenJK CG_Player modelScale semantics. `iModelScale` is a uniform
/// percent scale for networked NPC/player Ghoul2 models. The renderer applies
/// it to all three entity axes. Non-vehicle characters also receive the legacy
/// vertical correction so small models (notably Jawas) keep their feet on the
/// floor instead of shrinking around their origin.
fn ghoul2_render_transform(
    entity: &PresentedEntity,
    mut axis: [[f32; 3]; 3],
) -> ([[f32; 3]; 3], [f32; 3]) {
    let scale_int = entity.state.field_i32("iModelScale").unwrap_or(0);
    let mut origin = entity.origin;
    if scale_int == 0 {
        return (axis, origin);
    }

    let scale = scale_int as f32 / 100.0;
    if scale != 1.0 {
        for basis in &mut axis {
            for component in basis {
                *component *= scale;
            }
        }
        if entity.state.field_i32("NPC_class").unwrap_or(0) != crate::cgame::CLASS_VEHICLE {
            origin[2] += 24.0 * (scale - 1.0);
        }
    }
    (axis, origin)
}

fn ghoul2_render_origin(entity: &PresentedEntity) -> [f32; 3] {
    let (_, origin) = ghoul2_render_transform(entity, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    origin
}

/// Faithful G2_ComputeLOD shape with OpenJK `r_lodscale` (default 5) and
/// r_autolodscalevalue=0. `global_lod_bias` is the archived JKA `r_lodbias`
/// cvar. Per-model mLodBias is not authored by our current GLM presentation
/// state, so it is zero; OpenJK therefore effectively uses max(r_lodbias, 0).
fn ghoul2_lod_for_view(
    view: Ghoul2PresentationView,
    entity: &PresentedEntity,
    render_origin: [f32; 3],
    num_lods: usize,
    global_lod_bias: i32,
    lod_scale: f32,
) -> usize {
    if num_lods < 2 {
        return 0;
    }
    let scaled_radius = 0.75 * ghoul2_entity_radius(entity) * ghoul2_model_scale(entity);
    let projected_radius = view.projected_radius(render_origin, scaled_radius);
    let flod = if projected_radius != 0.0 {
        1.0 - projected_radius * lod_scale
    } else {
        0.0
    };
    // OpenJK x64 Q_ftol uses cvttss2si: truncate toward zero, clamps the
    // projected LOD, then adds max(r_lodbias, modelBias) and clamps again.
    let max_lod = num_lods.saturating_sub(1) as isize;
    let lod = ((flod * num_lods as f32) as isize).clamp(0, max_lod);
    let lod_bias = global_lod_bias.max(0) as isize;
    (lod + lod_bias).clamp(0, max_lod) as usize
}

#[derive(Debug)]
struct Ghoul2GpuMeshSource {
    /// Original GLM geometry key/topology used whenever jiggle is off.
    base_key: Arc<str>,
    base_vertices: Arc<Vec<Ghoul2GpuVertex>>,
    cpu_indices: Arc<Vec<u32>>,
    /// GPU/promoted topology. Jiggle surfaces use one Phong-style subdivision.
    key: Arc<str>,
    vertices: Arc<Vec<Ghoul2GpuVertex>>,
    indices: Arc<Vec<u32>>,
}

#[derive(Clone)]
struct SurfaceOverlay {
    texture: Option<Arc<TextureData>>,
    alpha_mode: DynamicModelAlphaMode,
}

/// One stage of a fully blended (additive/glow) shader, drawn as its own
/// pass over the surface: OpenJK runs every stage of a `trans` player shader, so
/// the first stage alone is usually not the look the author built.
#[derive(Clone)]
struct ResolvedStage {
    texture: Option<Arc<TextureData>>,
    alpha_mode: DynamicModelAlphaMode,
    /// `rgbGen const` colour (white otherwise) and `alphaGen const` alpha.
    rgb: [f32; 3],
    alpha: f32,
    /// `rgbGen const` stages are not lit by the light grid.
    unlit: bool,
    /// `alphaGen lightingSpecular`: alpha is q3's specular term per vertex.
    specular_alpha: bool,
    /// Only `tcMod scale` / `tcMod scroll` are applied.
    tc_mods: Vec<TcMod>,
}

/// A surface's resolved material: the primary stage plus the entity-tint layers.
struct ResolvedMaterial {
    texture: Option<Arc<TextureData>>,
    alpha_mode: DynamicModelAlphaMode,
    entity_tint: bool,
    overlay: Option<SurfaceOverlay>,
    /// Non-empty for a blended multi-stage shader; then `texture`/`alpha_mode`
    /// are the first entry's and the surface is drawn once per stage.
    stages: Vec<ResolvedStage>,
    /// `cull twosided` on a translucent surface (wings): the mesh also needs its
    /// reversed-winding triangles because the pipelines always cull back faces.
    two_sided: bool,
}

#[derive(Clone)]
struct PlayerSurfaceAsset {
    surface_index: usize,
    /// Ghoul2/skin default state. Dismemberment keeps authored `*off` cap
    /// surfaces resident and force-enables them only after a server sever event.
    default_visible: bool,
    /// Ghoul2 hierarchy name. Kept so runtime surface on/off rules such as
    /// TaystJK cg_fpls can be applied without rebuilding the model.
    surface_name: String,
    texture: Option<Arc<TextureData>>,
    alpha_mode: DynamicModelAlphaMode,
    /// Asset Viewer only: this surface had no usable authored material, so
    /// render it neutral gray instead of disappearing into the black preview.
    fallback_gray: bool,
    /// The primary stage is `rgbGen lightingDiffuseEntity`: the entity's
    /// shaderRGBA (`char_color_*`) tints it.
    entity_tint: bool,
    /// The later alpha-blended stage of a tinted shader, redrawn untinted over the
    /// base: the base texture's alpha marks the regions that follow `char_color_*`.
    overlay: Option<SurfaceOverlay>,
    /// Every drawable stage of a blended multi-stage shader (see `ResolvedStage`).
    stages: Vec<ResolvedStage>,
    /// Static bind-pose mesh for each authored GLM LOD. A surface can be
    /// absent from a lower LOD, matching Ghoul2's per-LOD surface tables.
    gpu_meshes: Vec<Option<Arc<Ghoul2GpuMeshSource>>>,
}

const FPLS_MODE3_OFF_SURFACES: &[&str] = &[
    // EternalJK CG_ForceFPLSPlayerModel: every FPLS mode removes the head
    // variants and the TIE-pilot hoses so first person never sits inside them.
    "head_eyes_mouth",
    "heada_eyes_mouth",
    "head",
    "heada",
    "heada_face",
    "headb",
    "headb_face",
    "headb_eyes_mouth",
    "torso_l_hose",
    "torso_r_hose",
    // EternalJK modes 2/3 additionally turn these exact surfaces off; its
    // source describes that state as "removes everything but the saber hilt".
    // Dinurdo's cg_fpls is boolean, so enabled maps to mode 3: no player body,
    // normal first-person camera, held saber hilt/blades retained.
    "hips",
    "hipsa",
    "torso",
    "torsoa",
];

fn fpls_mode3_surface_hidden(name: &str) -> bool {
    FPLS_MODE3_OFF_SURFACES
        .iter()
        .any(|surface| name.eq_ignore_ascii_case(surface))
}

struct PlayerModelAsset {
    key: String,
    glm: Arc<GlmModel>,
    gla: Arc<GlaAnimation>,
    surfaces: Vec<PlayerSurfaceAsset>,
    /// Optional post-Ghoul2 soft-tissue profile loaded beside model.glm.
    jiggle: Option<Arc<JiggleProfile>>,
}

struct SaberModelAsset {
    glm: Arc<GlmModel>,
    gla: Arc<GlaAnimation>,
    surfaces: Vec<PlayerSurfaceAsset>,
}

#[derive(Clone, Copy, Debug)]
struct BodyQueueCopyState {
    source_client: u16,
    _known_weapon: i32,
    _light_side: bool,
    /// Ghoul2 model index 1 after CG_BodyQueueCopy has duplicated the source
    /// instance and applied its knownWeapon correction. This is deliberately a
    /// g2WeaponInstances-style weapon identity: for ET_BODY, WP_SABER resolves
    /// to OpenJK's default saber instance rather than the live client's custom
    /// primary saber instance.
    model1_weapon: Option<i32>,
    /// Ghoul2 model index 2 survives the body copy independently. OpenJK uses
    /// it for the second saber and does not strip it with the model-1 rule.
    model2_saber: bool,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum DismemberPart {
    Head,
    Waist,
    LeftArm,
    RightArm,
    RightHand,
    LeftLeg,
    RightLeg,
}

#[derive(Debug, Clone, Copy)]
struct DismemberSpec {
    /// Ghoul2 bone OpenJK uses to position/orient the detached copy.
    rotate_bone: &'static str,
    limb_root: &'static str,
    stub_root: &'static str,
    limb_tag: &'static str,
    stub_tag: &'static str,
    /// Bone endpoints used only for the single cheap Rapier collider.
    collider_a: &'static str,
    collider_b: Option<&'static str>,
    radius: f32,
    mass_kg: f32,
}

impl DismemberPart {
    fn from_model_part(value: i32) -> Option<Self> {
        Some(match value {
            G2_MODELPART_HEAD => Self::Head,
            G2_MODELPART_WAIST => Self::Waist,
            G2_MODELPART_LARM => Self::LeftArm,
            G2_MODELPART_RARM => Self::RightArm,
            G2_MODELPART_RHAND => Self::RightHand,
            G2_MODELPART_LLEG => Self::LeftLeg,
            G2_MODELPART_RLEG => Self::RightLeg,
            _ => return None,
        })
    }

    fn model_part(self) -> i32 {
        match self {
            Self::Head => G2_MODELPART_HEAD,
            Self::Waist => G2_MODELPART_WAIST,
            Self::LeftArm => G2_MODELPART_LARM,
            Self::RightArm => G2_MODELPART_RARM,
            Self::RightHand => G2_MODELPART_RHAND,
            Self::LeftLeg => G2_MODELPART_LLEG,
            Self::RightLeg => G2_MODELPART_RLEG,
        }
    }

    fn allowed_at_level(self, level: u8) -> bool {
        level >= 2 || (level >= 1 && !matches!(self, Self::Head | Self::Waist))
    }

    fn spec(self) -> DismemberSpec {
        match self {
            // Exact OpenJK CG_General names/tags. Collider bones are a modern
            // Rapier-only approximation and never feed gameplay or sever choice.
            Self::Head => DismemberSpec {
                rotate_bone: "cranium",
                limb_root: "head",
                stub_root: "torso",
                limb_tag: "*head_cap_torso",
                stub_tag: "*torso_cap_head",
                collider_a: "cranium",
                collider_b: None,
                radius: 5.5,
                mass_kg: 5.0,
            },
            Self::Waist => DismemberSpec {
                rotate_bone: "thoracic",
                limb_root: "torso",
                stub_root: "hips",
                limb_tag: "*torso_cap_hips",
                stub_tag: "*hips_cap_torso",
                collider_a: "lower_lumbar",
                collider_b: Some("cranium"),
                radius: 7.0,
                mass_kg: 30.0,
            },
            Self::LeftArm => DismemberSpec {
                rotate_bone: "lradius",
                limb_root: "l_arm",
                stub_root: "torso",
                limb_tag: "*l_arm_cap_torso",
                stub_tag: "*torso_cap_l_arm",
                collider_a: "lhumerus",
                collider_b: Some("lhand"),
                radius: 3.2,
                mass_kg: 4.3,
            },
            Self::RightArm => DismemberSpec {
                rotate_bone: "rradius",
                limb_root: "r_arm",
                stub_root: "torso",
                limb_tag: "*r_arm_cap_torso",
                stub_tag: "*torso_cap_r_arm",
                collider_a: "rhumerus",
                collider_b: Some("rhand"),
                radius: 3.2,
                mass_kg: 4.3,
            },
            Self::RightHand => DismemberSpec {
                rotate_bone: "rhand",
                limb_root: "r_hand",
                stub_root: "r_arm",
                limb_tag: "*r_hand_cap_r_arm",
                stub_tag: "*r_arm_cap_r_hand",
                collider_a: "rhand",
                collider_b: None,
                radius: 2.8,
                mass_kg: 0.7,
            },
            Self::LeftLeg => DismemberSpec {
                rotate_bone: "ltibia",
                limb_root: "l_leg",
                stub_root: "hips",
                limb_tag: "*l_leg_cap_hips",
                stub_tag: "*hips_cap_l_leg",
                collider_a: "lfemurYZ",
                collider_b: Some("ltalus"),
                radius: 4.0,
                mass_kg: 12.0,
            },
            Self::RightLeg => DismemberSpec {
                rotate_bone: "rtibia",
                limb_root: "r_leg",
                stub_root: "hips",
                limb_tag: "*r_leg_cap_hips",
                stub_tag: "*hips_cap_r_leg",
                collider_a: "rfemurYZ",
                collider_b: Some("rtalus"),
                radius: 4.0,
                mass_kg: 12.0,
            },
        }
    }

    fn removes_weapon(self) -> bool {
        matches!(self, Self::Waist | Self::RightArm | Self::RightHand)
    }
}

#[derive(Clone)]
struct DismemberSourceSnap {
    model: Arc<PlayerModelAsset>,
    pose: Vec<Matrix3x4>,
    axis: [[f32; 3]; 3],
    origin: [f32; 3],
    model_scale: f32,
    body_rgba: [f32; 4],
    body_rgb: [f32; 3],
    /// Ghoul2 model slot 1 as it existed before CG_General mutates the source.
    /// OpenJK duplicates this slot onto the detached copy after first removing
    /// model slot 2 (the second saber) and model slot 3 (jetpack).
    model1_weapon: Option<i32>,
    model1_primary_saber: bool,
    client_info: crate::cgame::ClientInfo,
}

#[derive(Clone)]
struct DetachedLimbVisual {
    generation: DetachedLimbGeneration,
    source_entity: u16,
    part: DismemberPart,
    model: Arc<PlayerModelAsset>,
    pose: Vec<Matrix3x4>,
    surfaces: HashSet<usize>,
    spawn_entity_matrix: Matrix3x4,
    spawn_body_matrix: Matrix3x4,
    body_rgba: [f32; 4],
    body_rgb: [f32; 3],
    model1_weapon: Option<i32>,
    model1_primary_saber: bool,
    client_info: crate::cgame::ClientInfo,
    next_smoke_time: i32,
}

fn dismember_source_entity(entity: &PresentedEntity) -> Option<u16> {
    if entity.entity_type != ET_GENERAL
        || entity.state.field_i32("weapon").unwrap_or(0) != G2_MODEL_PART
        || DismemberPart::from_model_part(entity.state.field_i32("modelGhoul2").unwrap_or(0)).is_none()
    {
        return None;
    }
    let model_index = entity.state.field_i32("modelindex").unwrap_or(-1);
    let source = if model_index >= 0 {
        model_index
    } else {
        entity.state.field_i32("otherEntityNum2").unwrap_or(-1)
    };
    u16::try_from(source).ok()
}

fn dismember_generation(entity: &PresentedEntity, source_entity: u16, part: DismemberPart) -> DetachedLimbGeneration {
    DetachedLimbGeneration {
        source_entity,
        kind: part.model_part() as u8,
        trajectory_time: entity.state.field_i32("pos.trTime").unwrap_or(0),
    }
}

fn dismember_source_ready(
    source_entity: &PresentedEntity,
    presented_torso_anim: Option<i32>,
) -> bool {
    // Stock CG_General waits until the owner's EF_DEAD and death animation have
    // reached the client before cloning the Ghoul2 instance. Without this gate,
    // a newly received model-part entity can visually sever a still-living pose.
    if source_entity.state.field_i32("eFlags").unwrap_or(0) & EF_DEAD == 0 {
        return false;
    }

    let state_anim = source_entity.state.field_i32("torsoAnim").unwrap_or(0) & !ANIM_TOGGLEBIT;
    let chopped_hand = animation_index("BOTH_RIGHTHANDCHOPPEDOFF")
        .and_then(|index| i32::try_from(index).ok());
    if chopped_hand == Some(state_anim) {
        return true;
    }

    let Some(presented_anim) = presented_torso_anim else {
        // CG_General also requires clEnt->pe.torso.animationNumber to be a
        // death animation. If this source has not been presented yet, wait one
        // frame rather than assuming its lerp-frame state already caught up.
        return false;
    };
    let presented_anim = presented_anim & !ANIM_TOGGLEBIT;
    is_death_animation(state_anim) && is_death_animation(presented_anim)
}

fn hierarchy_surface_index(glm: &GlmModel, name: &str) -> Option<usize> {
    glm.hierarchy
        .iter()
        .position(|surface| surface.name.eq_ignore_ascii_case(name))
}

/// OpenJK asks `BG_GetRootSurfNameWithVariant` for limb/stub roots. Player
/// skins use one-letter root variants (`r_arma`, `torsoa`, `hipsa`, ...), while
/// `_1`/`_2` suffixes are LOD surface names and must not be mistaken for a skin
/// variant. Prefer the unsuffixed stock root, then accept exactly one ASCII
/// alphabetic variant character.
fn resolve_dismember_root(glm: &GlmModel, base: &str) -> Option<usize> {
    hierarchy_surface_index(glm, base).or_else(|| {
        let base = base.to_ascii_lowercase();
        glm.hierarchy.iter().position(|surface| {
            let name = surface.name.to_ascii_lowercase();
            let Some(suffix) = name.strip_prefix(&base) else { return false };
            suffix.len() == 1 && suffix.as_bytes()[0].is_ascii_alphabetic()
        })
    })
}

fn collect_surface_subtree(glm: &GlmModel, root: usize, out: &mut HashSet<usize>) {
    if !out.insert(root) {
        return;
    }
    if let Some(surface) = glm.hierarchy.get(root) {
        for &child in &surface.children {
            collect_surface_subtree(glm, child, out);
        }
    }
}

fn variant_cap_name(glm: &GlmModel, root: usize, base_root: &str, default_tag: &str) -> String {
    let default_cap = default_tag.trim_start_matches('*');
    let Some(root_name) = glm.hierarchy.get(root).map(|surface| surface.name.as_str()) else {
        return default_cap.to_owned();
    };
    if root_name.eq_ignore_ascii_case(base_root) {
        return default_cap.to_owned();
    }
    let suffix = default_cap.strip_prefix(base_root).unwrap_or(default_cap);
    format!("{root_name}{suffix}")
}

fn default_surface_set(model: &PlayerModelAsset) -> HashSet<usize> {
    model
        .surfaces
        .iter()
        .filter(|surface| surface.default_visible)
        .map(|surface| surface.surface_index)
        .collect()
}

fn source_dismember_surface_set(
    model: &PlayerModelAsset,
    parts: Option<&HashSet<DismemberPart>>,
) -> Option<HashSet<usize>> {
    let parts = parts.filter(|parts| !parts.is_empty())?;
    let mut visible = default_surface_set(model);
    let drawable = model.surfaces.iter().map(|surface| surface.surface_index).collect::<HashSet<_>>();
    for part in parts {
        let spec = part.spec();
        let Some(limb_root) = resolve_dismember_root(&model.glm, spec.limb_root) else { continue };
        let mut hidden = HashSet::new();
        collect_surface_subtree(&model.glm, limb_root, &mut hidden);
        visible.retain(|index| !hidden.contains(index));

        let Some(stub_root) = resolve_dismember_root(&model.glm, spec.stub_root) else { continue };
        let cap = variant_cap_name(&model.glm, stub_root, spec.stub_root, spec.stub_tag);
        if let Some(cap_index) = hierarchy_surface_index(&model.glm, &cap).filter(|index| drawable.contains(index)) {
            visible.insert(cap_index);
        }
    }
    Some(visible)
}

fn detached_limb_surface_set(
    model: &PlayerModelAsset,
    current: DismemberPart,
    all_parts: Option<&HashSet<DismemberPart>>,
) -> HashSet<usize> {
    let spec = current.spec();
    let drawable = model.surfaces.iter().map(|surface| surface.surface_index).collect::<HashSet<_>>();
    let Some(root) = resolve_dismember_root(&model.glm, spec.limb_root) else { return HashSet::new() };
    let mut subtree = HashSet::new();
    collect_surface_subtree(&model.glm, root, &mut subtree);
    let mut visible = model
        .surfaces
        .iter()
        .filter(|surface| surface.default_visible && subtree.contains(&surface.surface_index))
        .map(|surface| surface.surface_index)
        .collect::<HashSet<_>>();

    let cap = variant_cap_name(&model.glm, root, spec.limb_root, spec.limb_tag);
    if let Some(cap_index) = hierarchy_surface_index(&model.glm, &cap).filter(|index| drawable.contains(index)) {
        visible.insert(cap_index);
    }

    // The detached Ghoul2 copy inherits older sever flags from the source.
    if let Some(parts) = all_parts {
        for part in parts.iter().copied().filter(|part| *part != current) {
            let prior = part.spec();
            if let Some(prior_root) = resolve_dismember_root(&model.glm, prior.limb_root) {
                let mut hidden = HashSet::new();
                collect_surface_subtree(&model.glm, prior_root, &mut hidden);
                visible.retain(|index| !hidden.contains(index));
            }
        }
    }
    visible
}

fn presentation_matrix(axis: [[f32; 3]; 3], origin: [f32; 3]) -> Matrix3x4 {
    [
        [axis[0][0], axis[1][0], axis[2][0], origin[0]],
        [axis[0][1], axis[1][1], axis[2][1], origin[1]],
        [axis[0][2], axis[1][2], axis[2][2], origin[2]],
    ]
}

fn scaled_bone_matrix(mut matrix: Matrix3x4, scale: f32) -> Matrix3x4 {
    matrix[0][3] *= scale;
    matrix[1][3] *= scale;
    matrix[2][3] *= scale;
    matrix
}

fn affine_inverse_3x4(matrix: &Matrix3x4) -> Option<Matrix3x4> {
    let m = glam::Mat4::from_cols(
        glam::Vec4::new(matrix[0][0], matrix[1][0], matrix[2][0], 0.0),
        glam::Vec4::new(matrix[0][1], matrix[1][1], matrix[2][1], 0.0),
        glam::Vec4::new(matrix[0][2], matrix[1][2], matrix[2][2], 0.0),
        glam::Vec4::new(matrix[0][3], matrix[1][3], matrix[2][3], 1.0),
    );
    if m.determinant().abs() < 1.0e-8 {
        return None;
    }
    let inv = m.inverse();
    Some([
        [inv.x_axis.x, inv.y_axis.x, inv.z_axis.x, inv.w_axis.x],
        [inv.x_axis.y, inv.y_axis.y, inv.z_axis.y, inv.w_axis.y],
        [inv.x_axis.z, inv.y_axis.z, inv.z_axis.z, inv.w_axis.z],
    ])
}

fn presentation_axis_origin(matrix: &Matrix3x4) -> ([[f32; 3]; 3], [f32; 3]) {
    (
        [
            [matrix[0][0], matrix[1][0], matrix[2][0]],
            [matrix[0][1], matrix[1][1], matrix[2][1]],
            [matrix[0][2], matrix[1][2], matrix[2][2]],
        ],
        [matrix[0][3], matrix[1][3], matrix[2][3]],
    )
}

fn affine_point(matrix: &Matrix3x4, point: [f32; 3]) -> [f32; 3] {
    [
        matrix[0][0] * point[0] + matrix[0][1] * point[1] + matrix[0][2] * point[2] + matrix[0][3],
        matrix[1][0] * point[0] + matrix[1][1] * point[1] + matrix[1][2] * point[2] + matrix[1][3],
        matrix[2][0] * point[0] + matrix[2][1] * point[1] + matrix[2][2] * point[2] + matrix[2][3],
    ]
}

/// OpenJK stores current/desired blade length in clientInfo_t::saber[].blade[].
/// Keep the same persistent presentation state so EF_DEAD can retract a live
/// player's blade instead of snapping it back to its authored full length.
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct SaberBladeLengthKey {
    client_num: usize,
    saber_num: u8,
    blade_num: u8,
}

#[derive(Clone, Copy, Debug)]
struct SaberBladeLengthState {
    length: f32,
    length_max: f32,
    desired_length: f32,
    extend_debounce: i32,
    last_update_time: i32,
}

impl SaberBladeLengthState {
    fn new(length_max: f32, desired_length: f32, time: i32) -> Self {
        let length_max = length_max.max(0.0);
        let length = if desired_length == 0.0 { 0.0 } else { length_max };
        Self {
            length,
            length_max,
            desired_length,
            extend_debounce: time,
            last_update_time: time,
        }
    }

    /// Direct arithmetic port of OpenJK BG_SI_SetLengthGradual for one blade.
    fn set_desired_and_update(&mut self, desired_length: f32, length_max: f32, time: i32) -> f32 {
        self.length_max = length_max.max(0.0);
        self.length = self.length.clamp(0.0, self.length_max);
        self.desired_length = desired_length;
        if self.last_update_time == time {
            return self.length;
        }
        self.last_update_time = time;

        let desired = if self.desired_length == -1.0 {
            self.length_max
        } else {
            self.desired_length.clamp(0.0, self.length_max)
        };
        if self.length == desired {
            return self.length;
        }
        if self.length == self.length_max || self.length == 0.0 {
            self.extend_debounce = time;
            if self.length == 0.0 {
                self.length += 1.0;
            } else {
                self.length -= 1.0;
            }
        }
        let mut amount = (time - self.extend_debounce) as f32 * 0.01;
        if amount < 0.2 {
            amount = 0.2;
        }
        if self.length < desired {
            self.length += amount;
            if self.length > desired {
                self.length = desired;
            }
            if self.length > self.length_max {
                self.length = self.length_max;
            }
        } else if self.length > desired {
            self.length -= amount;
            if self.length < desired {
                self.length = desired;
            }
            if self.length < 0.0 {
                self.length = 0.0;
            }
        }
        self.length
    }
}

struct EntityPlayerState {
    model_key: String,
    /// False while `model` is only a stand-in (previous model or default
    /// fallback) for a requested model that is still loading. A settled state
    /// with matching requested names lets the per-frame lookup be skipped.
    model_settled: bool,
    // Cache the resolved model directly on the persistent centity-like state.
    // This avoids rebuilding/lowercasing qpath cache keys and hashing the global
    // model cache for every visible player on every frame.
    requested_model_name: String,
    requested_skin_name: String,
    model: Arc<PlayerModelAsset>,
    animation: PlayerAnimationState,
    // OpenJK cent->pe + clientInfo angle state consumed by BG_G2PlayerAngles.
    player_angles: PlayerAngleState,
    last_angle_time: i32,
    last_e_flags: i32,
    last_client_num: i32,
    /// `cent->ghoul2weapon`: which g2WeaponInstances entry was last copied.
    ghoul2_weapon: Option<i32>,
    /// OpenJK `cent->weapon`: identity of the weapon model(s) currently bolted
    /// into the persistent Ghoul2 instance. This intentionally survives death
    /// even when currentState.weapon changes; EV_DESTROY_WEAPON_MODEL or a
    /// later live weapon swap is what changes the bolted model state.
    cent_weapon: i32,
    /// Ghoul2 model index 1 on the player when it is a non-saber weapon.
    attached_weapon: Option<i32>,
    /// Ghoul2 model index 1 when occupied by the primary saber hilt.
    primary_saber_attached: bool,
    /// Ghoul2 model index 2 when occupied by the second saber hilt.
    secondary_saber_attached: bool,
    /// `clientInfo_t::saberName/saber2Name` as of the last frame, so a userinfo
    /// change can force CG_NewClientInfo's weapon-instance refresh.
    saber_names: [String; 2],
    /// jaPRO `CBoneCache::mSmoothBones`: the previous frame's filtered final
    /// bone pose, consumed by `r_ghoul2animsmooth`. `None` when there is no
    /// valid continuous history (first frame, a visibility gap, ragdoll, or a
    /// model swap), matching jaPRO's `touch != mLastTouch` reseed case.
    bone_smooth_history: Option<Vec<Matrix3x4>>,
    /// `current_time` of the last `bone_smooth_history` update. Frustum
    /// culling returns early without touching the history above, so staleness
    /// is checked by elapsed time rather than relying on every early return to
    /// invalidate it.
    bone_smooth_time: i32,
}

/// One cosmetic MD3 (`CG_DrawCosmeticOnPlayer`): a hat or cape bolted to a player.
#[derive(Clone, Debug, PartialEq)]
pub struct CosmeticDraw {
    pub entity_num: u16,
    pub model: &'static str,
    pub origin: [f32; 3],
    pub axis: [[f32; 3]; 3],
    pub rgba: [f32; 4],
    /// The parent's customShader (ghost racers carry their cosmetics with them).
    pub custom_shader: Option<&'static str>,
}

/// Force-power visuals CG_Player hands to the FX system / local entities.
#[derive(Clone, Debug, PartialEq)]
pub enum PlayerFxRequest {
    /// FX_PlayEntityEffectID at a hand bolt (lightning, drain).
    Effect { name: &'static str, origin: [f32; 3], axis: [[f32; 3]; 3] },
    /// An authored effect (by name) played along `dir` (vehicle muzzle flashes).
    EffectDir { name: String, origin: [f32; 3], dir: [f32; 3] },
    /// CG_DoSaber blade presentation. The FX layer owns the view-facing geometry
    /// so authored saber shaders go through the same material path as other FX.
    SaberBlade {
        origin: [f32; 3],
        direction: [f32; 3],
        length: f32,
        /// Authored `lengthMax`; CG_DoSaber widens the blade while `length`
        /// is still below it (ignition / retraction halo).
        length_max: f32,
        radius: f32,
        color: i32,
        entity_alpha: f32,
        /// Identity/state needed by OpenJK CG_AddSaberBlade's persistent
        /// per-blade trail history.
        entity_num: u16,
        saber_num: u8,
        blade_num: u8,
        saber_move: i32,
        torso_anim: i32,
        saber_in_flight: bool,
        trail_style: i32,
        /// Number of authored blades on this saber. OpenJK uses one combined
        /// dynamic light for sabers with 3+ blades instead of one per blade.
        num_blades: u8,
        /// Authored OpenJK `noDlight` / SFL2_NO_DLIGHT.
        no_dlight: bool,
        /// Authored OpenJK noWallMarks/noWallMarks2 for the active blade style.
        no_wall_marks: bool,
    },
    /// CG_ForcePushBlur's LE_PUFF path (two sprites drifting sideways).
    PushPuffs { origin: [f32; 3] },
    /// CG_ForceGripEffect (a red puff and a red saber-glow puff).
    GripPuffs { origin: [f32; 3] },
    /// CG_PlayerFloatSprite: an icon above a player's head (chat balloon, voice
    /// chat, connection trouble), white and 10 units in radius.
    HeadSprite { origin: [f32; 3], shader: &'static str },
}

const EF_BODYPUSH: i32 = 1 << 19;
const EF_JETPACK_ACTIVE: i32 = 1 << 11;
const EF_JETPACK: i32 = 1 << 29;
const EF_JETPACK_FLAMING: i32 = 1 << 30;
const JAPRO_CINFO2_WTTRIBES: i32 = 1 << 4;
const JETPACK_MODEL: &str = "models/weapons2/jetpack/model.glm";
const PW_DISINT_4: i32 = 9;
const FP_GRIP: i32 = 6;
const MAX_GRIP_DISTANCE: f32 = 256.0;
const DEFAULT_VIEWHEIGHT: f32 = 26.0;
const DEFAULT_PLAYER_MINS: [f32; 3] = [-15.0, -15.0, -24.0];
const DEFAULT_PLAYER_MAXS: [f32; 3] = [15.0, 15.0, 40.0];
const FP_RAGE: i32 = 8;
const FP_PROTECT: i32 = 9;
const FORCE_LEVEL_2: i32 = 2;
const FORCE_LEVEL_3: i32 = 3;
/// cg_pushBoneNames for CG_ForcePushBodyBlur.
const PUSH_BONE_NAMES: [&str; 8] = ["cranium", "lower_lumbar", "rhand", "lhand", "ltibia", "rtibia", "lradius", "rradius"];

const MAX_CLIENTS: usize = 32;
const MAX_SABER_BLADES: usize = 8;
/// WP_SaberSetDefaults: `blade[].lengthMax` before a definition overrides it.
const DEFAULT_SABER_BLADE_LENGTH_MAX: f32 = 32.0;

/// weapon_t values used by the held-weapon attachment rules.
const WP_MELEE: i32 = 2;
const WP_EMPLACED_GUN: i32 = 17;
const TEAM_SPECTATOR: i32 = 3;

/// The saber-specific weapon bootstrap at the end of OpenJK's
/// CG_ResetPlayerEntity. A remote player entering the PVS (including every
/// entity in an initial demo snapshot) copies the complete WP_SABER Ghoul2
/// weapon instance before CG_Player applies saberInFlight. The copy itself
/// runs CG_CopyG2WeaponInstance, which places saber 0 at model index 1 and
/// saber 1 at model index 2. This is deliberately separate from the normal
/// per-frame weapon-pointer comparison below: ghoul2weapon identifies the
/// primary weapon instance and is not evidence that model index 2 was copied.
#[allow(clippy::too_many_arguments)]
fn reset_remote_player_saber_attachment(
    ghoul2_weapon: &mut Option<i32>,
    cent_weapon: &mut i32,
    attached_weapon: &mut Option<i32>,
    primary_saber_attached: &mut bool,
    secondary_saber_attached: &mut bool,
    requested_weapon: i32,
    remote_from_predicted_player: bool,
    has_primary_saber: bool,
    has_secondary_saber: bool,
) {
    // cg_players.c::CG_ResetPlayerEntity:
    //   currentState.number != predictedPlayerState.clientNum
    //   && currentState.weapon == WP_SABER
    //   && cent->weapon != currentState.weapon
    if !remote_from_predicted_player
        || requested_weapon != WP_SABER
        || *cent_weapon == requested_weapon
    {
        return;
    }

    *cent_weapon = requested_weapon;

    // CG_CopyG2WeaponInstance(WP_SABER) copies both configured saber slots.
    // In this Rust presenter these booleans are the actual model-index state.
    *attached_weapon = None;
    *primary_saber_attached = has_primary_saber;
    *secondary_saber_attached = has_secondary_saber;
    *ghoul2_weapon = Some(WP_SABER);
}

/// OpenJK CG_Player + CG_CopyG2WeaponInstance. `ghoul2_weapon` is the last
/// g2WeaponInstances pointer identity, `cent_weapon` is the persistent centity
/// weapon identity, and the three model-slot arguments are the actual Ghoul2
/// attachment state. Death clears only ghoul2weapon; it does not
/// manufacture/remove held models.
#[allow(clippy::too_many_arguments)]
fn update_weapon_attachment(
    ghoul2_weapon: &mut Option<i32>,
    cent_weapon: &mut i32,
    attached_weapon: &mut Option<i32>,
    primary_saber_attached: &mut bool,
    secondary_saber_attached: &mut bool,
    requested_weapon: i32,
    instance: Option<i32>,
    saber_in_flight: bool,
    dead: bool,
    spectator: bool,
    has_primary_saber: bool,
    has_secondary_saber: bool,
) {
    // CG_Player recomputes whether Ghoul2 model index 1 actually exists every
    // frame. If it does not, OpenJK forcibly clears both tracking fields so a
    // weapon can be recopied later (cg_players.c: g2HasWeapon check immediately
    // before the weapon update block). This is essential for saber return: the
    // in-flight path removes model index 1 while leaving the logical saber
    // instance selected, and the missing-slot check is what makes the catch
    // reattach it on the next non-flight frame.
    let g2_has_weapon = attached_weapon.is_some() || *primary_saber_attached;
    if !g2_has_weapon {
        *ghoul2_weapon = None;
        *cent_weapon = 0;
    }

    // CG_Player does this after the actual model-slot test and before its
    // death/spectator tests so an in-flight primary saber cannot be
    // copied back into the hand until the server says it has returned.
    if saber_in_flight {
        *ghoul2_weapon = Some(WP_SABER);
    }

    if dead {
        // CG_Player / CG_CheckPlayerG2Weapons: "no updating weapons when dead". The
        // copied models stay exactly as they were until an explicit event or
        // body-copy operation changes them.
        *ghoul2_weapon = None;
        return;
    }

    if spectator {
        // OpenJK resets the tracking pointers for spectators, but does not
        // issue a Ghoul2 RemoveGhoul2Model here. Preserve the actual slots.
        *ghoul2_weapon = None;
        *cent_weapon = 0;
        return;
    }

    if *ghoul2_weapon == instance {
        return;
    }

    match instance {
        Some(WP_SABER) => {
            // CG_CopyG2WeaponInstance copies saber 0 to model index 1 and, for
            // dual sabers, saber 1 to model index 2.
            *attached_weapon = None;
            *primary_saber_attached = has_primary_saber;
            *secondary_saber_attached = has_secondary_saber;
        }
        Some(WP_MELEE | WP_EMPLACED_GUN) => {
            // These weapon instances deliberately remove held Ghoul2 models.
            *attached_weapon = None;
            *primary_saber_attached = false;
            *secondary_saber_attached = false;
        }
        Some(other) => {
            *attached_weapon = Some(other);
            *primary_saber_attached = false;
            *secondary_saber_attached = false;
        }
        // No g2WeaponInstances entry means CG_CopyG2WeaponInstance does
        // nothing; the previous actual model slots remain bolted on.
        None => {}
    }

    *cent_weapon = requested_weapon;
    *ghoul2_weapon = instance;
}

/// OpenJK CG_BodyQueueCopy first duplicates the source Ghoul2 instance and
/// only then applies this correction to model index 1. Crucially, a missing
/// source model is never manufactured from knownWeapon.
fn body_queue_model1_weapon(source_model1: Option<i32>, known_weapon: i32) -> Option<i32> {
    let source_model1 = source_model1?;
    if known_weapon > WP_BRYAR_PISTOL {
        return None;
    }
    if known_weapon == WP_SABER {
        return Some(WP_SABER);
    }
    // CG_G2WeaponInstance must exist for CopySpecificGhoul2Model to replace
    // model index 1. If it does not, retain the duplicated source slot.
    weapon_world_model(known_weapon).map_or(Some(source_model1), |_| Some(known_weapon))
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Ghoul2PerfStats {
    pub pose_ms: f64,
    pub motion_pose_ms: f64,
    pub skin_ms: f64,
    pub bolt_ms: f64,
    pub pose_evals: u32,
    pub motion_pose_evals: u32,
    pub bolt_queries: u32,
    pub surfaces_skinned: u32,
    pub vertices_skinned: u64,
    pub frustum_tests: u32,
    pub frustum_culled: u32,
    pub lod_counts: [u32; 4],
}

fn record_pose_eval(perf: &mut Ghoul2PerfStats, started: Instant) {
    perf.pose_ms += started.elapsed().as_secs_f64() * 1000.0;
    perf.pose_evals = perf.pose_evals.saturating_add(1);
}

fn record_motion_pose_eval(perf: &mut Ghoul2PerfStats, started: Instant) {
    // Keep the lazy pre-angle Motion chain separate from full final skeleton
    // evaluation. This makes the profiler prove that the old second full pose
    // is gone instead of reporting both requests under g2_pose.
    perf.motion_pose_ms += started.elapsed().as_secs_f64() * 1000.0;
    perf.motion_pose_evals = perf.motion_pose_evals.saturating_add(1);
}

/// A vehicle's last presented (static) pose, kept for muzzle bolts.
#[derive(Clone)]
struct VehicleSnap {
    /// Lower-case `.veh` name.
    vehicle: String,
    model: Arc<SaberModelAsset>,
    pose: Vec<Matrix3x4>,
    axis: [[f32; 3]; 3],
    origin: [f32; 3],
}

fn model_bolt_matrix_timed(
    perf: &mut Ghoul2PerfStats,
    glm: &GlmModel,
    gla: &GlaAnimation,
    pose: &[Matrix3x4],
    name: &str,
) -> Result<Option<Matrix3x4>, String> {
    let started = Instant::now();
    let result = model_bolt_matrix(glm, gla, pose, name);
    perf.bolt_ms += started.elapsed().as_secs_f64() * 1000.0;
    perf.bolt_queries = perf.bolt_queries.saturating_add(1);
    result
}

/// Profile is intentionally studio-lit rather than map-lightgrid-lit. This bakes
/// a key + soft fill from vertex normals into preview-only shaderRGBA. Gameplay
/// models continue through the configured lighting.
pub fn apply_profile_studio_light(draws: &mut [DynamicModelSurface]) {
    for surface in draws {
        let vertices = Arc::make_mut(&mut surface.vertices);
        for vertex in vertices.iter_mut() {
            let normal = Vec3::from_array(vertex.normal).normalize_or_zero();
            let key = normal.dot(Vec3::new(-0.35, 0.72, 0.60).normalize()).max(0.0);
            let fill = normal.dot(Vec3::new(0.55, 0.20, -0.35).normalize()).max(0.0);
            let light = (0.38 + 0.72 * key + 0.20 * fill).min(1.18);
            for channel in 0..3 {
                vertex.color[channel] *= light;
            }
        }
        // Do not also sample the current BSP lightgrid in the isolated
        // preview; the studio lighting should be stable on every map.
        surface.lighting_origin = None;
    }
}

/// `CG_DrawCosmeticOnPlayer` for every cosmetic in `mask`: bolt the hat MD3s to
/// `*head_top` and the capes/carried items to `*back`.
///
/// The bolt matrix here is the internal (low) one. The public
/// `G2API_GetBoltMatrix` jaPRO reads post-multiplies it by a 270-degree yaw
/// ("lots of game code is written to assume this 90 degree offset thing"), so
/// forward is -column 1, the second axis is column 0 and up is column 2. Using
/// the raw columns turns every cosmetic 90 degrees.
#[allow(clippy::too_many_arguments)]
fn cosmetic_draws_for_mask(
    perf: &mut Ghoul2PerfStats,
    model: &PlayerModelAsset,
    pose: &[Matrix3x4],
    axis: [[f32; 3]; 3],
    origin: [f32; 3],
    mask: u32,
    entity_num: u16,
    rgba: [f32; 4],
    custom_shader: Option<&'static str>,
) -> Vec<CosmeticDraw> {
    use crate::japro_cg::{self, CosmeticSlot};
    let mut out = Vec::new();
    for cosmetic in japro_cg::cosmetics_to_draw(mask) {
        let tag = match cosmetic.slot {
            CosmeticSlot::Hat => "*head_top",
            CosmeticSlot::Back => "*back",
        };
        let Ok(Some(m)) = model_bolt_matrix_timed(perf, &model.glm, &model.gla, pose, tag) else {
            continue;
        };
        let column = |index: usize, sign: f32| {
            transform_jka_model_vector([sign * m[0][index], sign * m[1][index], sign * m[2][index]], axis)
        };
        let columns = [column(1, -1.0), column(0, 1.0), column(2, 1.0)];
        let mut position = transform_jka_model_point([m[0][3], m[1][3], m[2][3]], axis, origin);
        // VectorMA(boltOrg, -2, re.axis[2], boltOrg)
        for (value, up) in position.iter_mut().zip(columns[2]) {
            *value -= 2.0 * up;
        }
        out.push(CosmeticDraw {
            entity_num,
            model: cosmetic.model,
            origin: position,
            axis: columns,
            rgba,
            custom_shader,
        });
    }
    out
}

/// `PM_FootSlopeTrace`'s two `G2API_GetBoltMatrix` queries, in model space
/// (scaled like G2's `scale` argument). `None` while the model has no foot tags.
fn foot_bolts_timed(
    perf: &mut Ghoul2PerfStats,
    model: &PlayerModelAsset,
    pose: &[Matrix3x4],
    scale: f32,
) -> Option<[[f32; 3]; 2]> {
    let mut foot = |name| {
        model_bolt_matrix_timed(perf, &model.glm, &model.gla, pose, name)
            .ok()
            .flatten()
            .map(|m| [m[0][3] * scale, m[1][3] * scale, m[2][3] * scale])
    };
    Some([foot("*l_leg_foot")?, foot("*r_leg_foot")?])
}

fn model_bolt_origin_timed(
    perf: &mut Ghoul2PerfStats,
    model: &PlayerModelAsset,
    pose: &[Matrix3x4],
    axis: [[f32; 3]; 3],
    entity_origin: [f32; 3],
    name: &str,
) -> Result<Option<[f32; 3]>, String> {
    Ok(model_bolt_matrix_timed(perf, &model.glm, &model.gla, pose, name)?
        .map(|m| transform_jka_model_point([m[0][3], m[1][3], m[2][3]], axis, entity_origin)))
}

fn skin_surface_timed(
    perf: &mut Ghoul2PerfStats,
    surface: &GlmSurface,
    pose: &[Matrix3x4],
) -> Result<Ghoul2SkinnedSurface, String> {
    let started = Instant::now();
    let result = skin_glm_surface(surface, pose);
    perf.skin_ms += started.elapsed().as_secs_f64() * 1000.0;
    if let Ok(skinned) = &result {
        perf.surfaces_skinned = perf.surfaces_skinned.saturating_add(1);
        perf.vertices_skinned = perf
            .vertices_skinned
            .saturating_add(skinned.vertices.len() as u64);
    }
    result
}

fn gpu_vertex_from_glm(
    surface: &GlmSurface,
    vertex_index: usize,
    jiggle_profile: Option<&JiggleProfile>,
    lod_index: usize,
) -> Result<Ghoul2GpuVertex, String> {
    let vertex = surface.vertices.get(vertex_index).ok_or_else(|| {
        format!("GLM surface {} missing vertex {vertex_index}", surface.surface_index)
    })?;
    let uv = *surface.texcoords.get(vertex_index).ok_or_else(|| {
        format!("GLM surface {} missing texcoord {vertex_index}", surface.surface_index)
    })?;
    if vertex.weights.is_empty() || vertex.weights.len() > 4 {
        return Err(format!(
            "GLM surface {} vertex {} has unsupported weight count {}",
            surface.surface_index, vertex_index, vertex.weights.len()
        ));
    }
    let mut bone_indices = [0u32; 4];
    let mut weights = [0.0f32; 4];
    for (weight_index, weight) in vertex.weights.iter().enumerate() {
        let skeleton_bone = *surface.bone_references.get(weight.local_bone_index).ok_or_else(|| {
            format!(
                "GLM surface {} vertex {} references missing local bone {}",
                surface.surface_index, vertex_index, weight.local_bone_index
            )
        })?;
        bone_indices[weight_index] = u32::try_from(skeleton_bone)
            .map_err(|_| format!("GLM skeleton bone index {skeleton_bone} exceeds GPU u32"))?;
        weights[weight_index] = weight.weight;
    }
    let (jiggle_region, jiggle_weight, jiggle_coord) = jiggle_profile
        .map(|profile| profile.gpu_vertex_binding(lod_index, surface.surface_index, vertex_index))
        .unwrap_or((u32::MAX, 0.0, 4.0));
    Ok(Ghoul2GpuVertex {
        position: vertex.position,
        normal: vertex.normal,
        uv,
        bone_indices,
        weights,
        weight_count: vertex.weights.len() as u32,
        jiggle_region,
        jiggle_weight,
        jiggle_coord,
    })
}

fn append_surface_indices(
    surface: &GlmSurface,
    two_sided: bool,
) -> Result<Vec<u32>, String> {
    let mut indices = Vec::with_capacity(surface.triangles.len() * 3 * if two_sided { 2 } else { 1 });
    for (triangle_index, triangle) in surface.triangles.iter().enumerate() {
        if triangle.iter().any(|&index| index as usize >= surface.vertices.len()) {
            return Err(format!(
                "GLM surface {} triangle {} has out-of-range vertex",
                surface.surface_index, triangle_index
            ));
        }
        let mut triangle = *triangle;
        triangle.swap(1, 2);
        indices.extend_from_slice(&triangle);
    }
    if two_sided {
        let front_count = indices.len();
        for triangle in 0..front_count / 3 {
            let base = triangle * 3;
            indices.extend_from_slice(&[indices[base], indices[base + 2], indices[base + 1]]);
        }
    }
    Ok(indices)
}

fn blend_gpu_influences(a: &Ghoul2GpuVertex, b: &Ghoul2GpuVertex) -> ([u32; 4], [f32; 4], u32) {
    let mut combined = HashMap::<u32, f32>::new();
    for source in [a, b] {
        for index in 0..source.weight_count.min(4) as usize {
            *combined.entry(source.bone_indices[index]).or_default() += source.weights[index] * 0.5;
        }
    }
    let mut influences = combined.into_iter().collect::<Vec<_>>();
    influences.sort_by(|left, right| right.1.total_cmp(&left.1));
    influences.truncate(4);
    let total = influences.iter().map(|(_, weight)| *weight).sum::<f32>().max(0.0001);
    let mut bone_indices = [0u32; 4];
    let mut weights = [0.0f32; 4];
    for (index, (bone, weight)) in influences.iter().enumerate() {
        bone_indices[index] = *bone;
        weights[index] = *weight / total;
    }
    (bone_indices, weights, influences.len().max(1) as u32)
}

fn promoted_midpoint(
    a: &Ghoul2GpuVertex,
    b: &Ghoul2GpuVertex,
    curve_position: bool,
) -> Ghoul2GpuVertex {
    let pa = Vec3::from_array(a.position);
    let pb = Vec3::from_array(b.position);
    let na = Vec3::from_array(a.normal).normalize_or_zero();
    let nb = Vec3::from_array(b.normal).normalize_or_zero();
    let linear = (pa + pb) * 0.5;
    let projected_a = linear - na * (linear - pa).dot(na);
    let projected_b = linear - nb * (linear - pb).dot(nb);
    let curved = if curve_position {
        linear.lerp((projected_a + projected_b) * 0.5, 0.75)
    } else {
        // Surface boundaries/UV seams must remain on the source edge so an
        // adjacent unpromoted surface cannot develop a crack.
        linear
    };
    let normal = (na + nb).normalize_or_zero();
    let (bone_indices, weights, weight_count) = blend_gpu_influences(a, b);
    let (jiggle_region, jiggle_weight, jiggle_coord) = match (a.jiggle_region, b.jiggle_region) {
        (ra, rb) if ra == rb => (
            ra,
            (a.jiggle_weight + b.jiggle_weight) * 0.5,
            (a.jiggle_coord + b.jiggle_coord) * 0.5,
        ),
        (u32::MAX, rb) => (rb, b.jiggle_weight * 0.5, b.jiggle_coord),
        (ra, u32::MAX) => (ra, a.jiggle_weight * 0.5, a.jiggle_coord),
        (ra, _) if a.jiggle_weight >= b.jiggle_weight => (ra, a.jiggle_weight * 0.5, a.jiggle_coord),
        (_, rb) => (rb, b.jiggle_weight * 0.5, b.jiggle_coord),
    };
    Ghoul2GpuVertex {
        position: curved.to_array(),
        normal: if normal.length_squared() > 0.0 { normal.to_array() } else { a.normal },
        uv: [(a.uv[0] + b.uv[0]) * 0.5, (a.uv[1] + b.uv[1]) * 0.5],
        bone_indices,
        weights,
        weight_count,
        jiggle_region,
        jiggle_weight,
        jiggle_coord,
    }
}

fn jiggle_promotion_weight(vertex: &Ghoul2GpuVertex) -> f32 {
    if vertex.jiggle_coord.abs() < 2.0 {
        // Match the default live glute-height trim while deciding where extra
        // topology is worth caching. This keeps upper-thigh triangles at stock
        // density even for older explicit model.jiggle files.
        let edge0 = -0.70 + 0.15;
        let edge1 = -0.15 + 0.15;
        let t = ((vertex.jiggle_coord - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
        return vertex.jiggle_weight * (t * t * (3.0 - 2.0 * t));
    }
    vertex.jiggle_weight
}

fn subdivide_jiggle_region(
    source_vertices: &[Ghoul2GpuVertex],
    source_indices: &[u32],
    threshold: f32,
) -> (Vec<Ghoul2GpuVertex>, Vec<u32>) {
    let selected = source_indices
        .chunks_exact(3)
        .map(|triangle| {
            triangle.iter().any(|&index| {
                source_vertices
                    .get(index as usize)
                    .is_some_and(|vertex| jiggle_promotion_weight(vertex) > threshold)
            })
        })
        .collect::<Vec<_>>();

    let mut vertices = source_vertices.to_vec();
    let mut edge_use = HashMap::<(u32, u32), u32>::new();
    for (triangle_index, triangle) in source_indices.chunks_exact(3).enumerate() {
        if !selected.get(triangle_index).copied().unwrap_or(false) {
            continue;
        }
        for (a, b) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ] {
            let key = if a < b { (a, b) } else { (b, a) };
            *edge_use.entry(key).or_default() += 1;
        }
    }

    // Create every edge midpoint required by a selected triangle before
    // emitting indices.  Unselected neighbours can then consume the exact same
    // midpoint and remain conforming instead of leaving a T-junction.
    let mut edges = HashMap::<(u32, u32), u32>::new();
    for (&key, &selected_uses) in &edge_use {
        let (a, b) = key;
        let (Some(va), Some(vb)) = (
            vertices.get(a as usize).copied(),
            vertices.get(b as usize).copied(),
        ) else {
            continue;
        };
        let index = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
        vertices.push(promoted_midpoint(&va, &vb, selected_uses > 1));
        edges.insert(key, index);
    }

    let edge_mid = |a: u32, b: u32| {
        let key = if a < b { (a, b) } else { (b, a) };
        edges.get(&key).copied()
    };
    let mut indices = Vec::with_capacity(source_indices.len() * 2);
    for (triangle_index, triangle) in source_indices.chunks_exact(3).enumerate() {
        let a = triangle[0];
        let b = triangle[1];
        let c = triangle[2];
        let ab = edge_mid(a, b);
        let bc = edge_mid(b, c);
        let ca = edge_mid(c, a);

        if selected.get(triangle_index).copied().unwrap_or(false) {
            let (Some(ab), Some(bc), Some(ca)) = (ab, bc, ca) else {
                indices.extend_from_slice(&[a, b, c]);
                continue;
            };
            indices.extend_from_slice(&[
                a, ab, ca,
                ab, b, bc,
                ca, bc, c,
                ab, bc, ca,
            ]);
            continue;
        }

        // Boundary neighbour: split only the edges that the promoted region
        // already introduced.  This does not add curvature or spread the high
        // density area, it only makes the transition watertight.
        match (ab, bc, ca) {
            (None, None, None) => indices.extend_from_slice(&[a, b, c]),
            (Some(ab), None, None) => {
                indices.extend_from_slice(&[a, ab, c, ab, b, c]);
            }
            (None, Some(bc), None) => {
                indices.extend_from_slice(&[a, b, bc, a, bc, c]);
            }
            (None, None, Some(ca)) => {
                indices.extend_from_slice(&[a, b, ca, b, c, ca]);
            }
            (Some(ab), Some(bc), None) => {
                indices.extend_from_slice(&[ab, b, bc, a, ab, bc, a, bc, c]);
            }
            (None, Some(bc), Some(ca)) => {
                indices.extend_from_slice(&[bc, c, ca, b, bc, ca, b, ca, a]);
            }
            (Some(ab), None, Some(ca)) => {
                indices.extend_from_slice(&[ca, a, ab, c, ca, ab, c, ab, b]);
            }
            (Some(ab), Some(bc), Some(ca)) => {
                indices.extend_from_slice(&[
                    a, ab, ca,
                    ab, b, bc,
                    ca, bc, c,
                    ab, bc, ca,
                ]);
            }
        }
    }
    (vertices, indices)
}

/// Two cached, local Phong-style subdivision passes.  Only triangles touching
/// the soft mask are promoted: the transition ring gets 4x topology and the
/// weighted core can reach 16x.  The rest of torso/hips remains original JKA.
fn promote_jiggle_mesh(
    source_vertices: &[Ghoul2GpuVertex],
    source_indices: &[u32],
) -> (Vec<Ghoul2GpuVertex>, Vec<u32>) {
    let (vertices, indices) = subdivide_jiggle_region(source_vertices, source_indices, 0.0125);
    subdivide_jiggle_region(&vertices, &indices, 0.045)
}

fn build_gpu_mesh_source(
    model_qpath: &str,
    lod_index: usize,
    surface: &GlmSurface,
    two_sided: bool,
    jiggle_profile: Option<&JiggleProfile>,
) -> Result<Arc<Ghoul2GpuMeshSource>, String> {
    if surface.vertices.len() != surface.texcoords.len() {
        return Err(format!(
            "GLM surface {} has {} vertices but {} texcoords",
            surface.surface_index, surface.vertices.len(), surface.texcoords.len()
        ));
    }
    let mut vertices = Vec::with_capacity(surface.vertices.len());
    for vertex_index in 0..surface.vertices.len() {
        vertices.push(gpu_vertex_from_glm(surface, vertex_index, jiggle_profile, lod_index)?);
    }

    let cpu_indices = append_surface_indices(surface, two_sided)?;
    let base_vertices = Arc::new(vertices.clone());
    let promote = jiggle_profile.is_some_and(|profile| {
        profile.gpu_supported() && profile.affects_surface(lod_index, surface.surface_index)
    });
    let (vertices, mut indices) = if promote {
        // Promote only the front winding; append mirrored triangles afterwards
        // for the rare two-sided material so midpoint topology is shared.
        let front_count = surface.triangles.len() * 3;
        let (vertices, mut promoted) = promote_jiggle_mesh(&vertices, &cpu_indices[..front_count]);
        if two_sided {
            let promoted_front = promoted.len();
            for triangle in 0..promoted_front / 3 {
                let base = triangle * 3;
                promoted.extend_from_slice(&[
                    promoted[base],
                    promoted[base + 2],
                    promoted[base + 1],
                ]);
            }
        }
        (vertices, promoted)
    } else {
        (vertices, cpu_indices.clone())
    };

    // Keep capacity tight after midpoint generation; these Arcs live with the
    // model asset for its whole residency.
    indices.shrink_to_fit();
    let base_key = Arc::<str>::from(format!(
        "{}#lod{}#surface{}{}",
        model_qpath.replace('\\', "/").to_ascii_lowercase(),
        lod_index,
        surface.surface_index,
        if two_sided { "#twosided" } else { "" },
    ));
    let key = if promote {
        Arc::<str>::from(format!("{base_key}#jiggle16xlocal"))
    } else {
        Arc::clone(&base_key)
    };
    let vertices = if promote { Arc::new(vertices) } else { Arc::clone(&base_vertices) };
    let cpu_indices = Arc::new(cpu_indices);
    let indices = if promote { Arc::new(indices) } else { Arc::clone(&cpu_indices) };
    Ok(Arc::new(Ghoul2GpuMeshSource {
        base_key,
        base_vertices,
        cpu_indices,
        key,
        vertices,
        indices,
    }))
}

fn build_lod_gpu_meshes(
    model_qpath: &str,
    glm: &GlmModel,
    surface_index: usize,
    two_sided: bool,
    jiggle_profile: Option<&JiggleProfile>,
) -> Result<Vec<Option<Arc<Ghoul2GpuMeshSource>>>, String> {
    glm.lods
        .iter()
        .enumerate()
        .map(|(lod_index, lod)| {
            lod.surfaces
                .iter()
                .find(|surface| surface.surface_index == surface_index)
                .map(|surface| {
                    build_gpu_mesh_source(
                        model_qpath,
                        lod_index,
                        surface,
                        two_sided,
                        jiggle_profile,
                    )
                })
                .transpose()
        })
        .collect()
}


fn gpu_bones_from_pose(pose: &[Matrix3x4]) -> Arc<Vec<Ghoul2GpuBone>> {
    Arc::new(
        pose.iter()
            .map(|matrix| Ghoul2GpuBone {
                row0: matrix[0],
                row1: matrix[1],
                row2: matrix[2],
            })
            .collect(),
    )
}

/// Marker carried by errors that only mean "still loading". Presentation code
/// treats it as "skip this frame", never as a failure to report.
const ASSET_PENDING: &str = "asset loading";

fn is_asset_pending(error: &str) -> bool {
    error.contains(ASSET_PENDING)
}

/// OpenJK cache key for a player's model + skin registration.
fn player_model_key(info: &crate::cgame::ClientInfo) -> String {
    format!("{}|{}", info.model_qpath(), info.skin_qpath()).to_ascii_lowercase()
}

fn sibling_default_skin_qpath(model_qpath: &str) -> Option<String> {
    let normalized = model_qpath.replace('\\', "/");
    let slash = normalized.rfind('/')?;
    Some(format!("{}model_default.skin", &normalized[..=slash]))
}

fn static_model_key(model_qpath: &str, custom_skin: Option<&str>, preview_fallback: bool) -> String {
    format!(
        "{}|{}|preview_fallback={}",
        model_qpath,
        custom_skin.unwrap_or(""),
        u8::from(preview_fallback),
    )
    .to_ascii_lowercase()
}

enum ModelLookup {
    Ready(Arc<PlayerModelAsset>),
    Pending,
    Failed(String),
    Missing,
}

enum ModelResolution {
    /// The requested model (or its terminal OpenJK fallback) is available.
    Ready(Arc<PlayerModelAsset>),
    /// Requested model still loading; this resident fallback body stands in.
    Provisional(Arc<PlayerModelAsset>),
    /// Nothing usable yet; the request is in flight.
    Loading,
    Failed(String),
}

/// Everything a Ghoul2 model registration needs from the outside world. The
/// synchronous path and the asset workers implement this differently, but
/// share exactly the same OpenJK registration logic in `build_*`.
trait ModelSource {
    fn read_model(&mut self, qpath: &str) -> Result<Vec<u8>, String>;
    fn read_optional(&mut self, qpath: &str, max_bytes: usize) -> Result<Option<Vec<u8>>, String>;
    fn load_gla(&mut self, qpath: &str) -> Result<Arc<GlaAnimation>, String>;
    fn load_skin(&mut self, qpath: &str) -> Result<Vec<jka_assets::skin::SkinSurface>, String>;
    fn resolve_material(&mut self, shader_name: &str) -> ResolvedMaterial;
}

/// Original behavior: reads and decodes on the calling thread, sharing the
/// presenter's texture cache.
struct SyncModelSource<'a> {
    presenter: &'a mut PlayerPresenter,
}

impl ModelSource for SyncModelSource<'_> {
    fn read_model(&mut self, qpath: &str) -> Result<Vec<u8>, String> {
        Ok(self
            .presenter
            .assets
            .read(qpath, MAX_GLM_BYTES)
            .map_err(|error| format!("{qpath}: {error}"))?
            .ok_or_else(|| format!("missing {qpath}"))?
            .bytes)
    }

    fn read_optional(&mut self, qpath: &str, max_bytes: usize) -> Result<Option<Vec<u8>>, String> {
        self.presenter
            .assets
            .read(qpath, max_bytes)
            .map_err(|error| format!("{qpath}: {error}"))
            .map(|asset| asset.map(|asset| asset.bytes))
    }

    fn load_gla(&mut self, qpath: &str) -> Result<Arc<GlaAnimation>, String> {
        self.presenter.load_glm_animation(qpath)
    }

    fn load_skin(&mut self, qpath: &str) -> Result<Vec<jka_assets::skin::SkinSurface>, String> {
        self.presenter.load_skin_qpath(qpath)
    }

    fn resolve_material(&mut self, shader_name: &str) -> ResolvedMaterial {
        self.presenter.resolve_surface_material(shader_name)
    }
}

/// Worker-side caches shared by every asset job of one presenter. Per-key
/// `OnceLock`s make concurrent jobs that need the same GLA or texture wait for
/// one decode instead of repeating it (all humanoid players share one GLA).
struct ModelLoadShared {
    shaders: Arc<BTreeMap<String, Shader>>,
    glas: Mutex<HashMap<String, Arc<OnceLock<Result<Arc<GlaAnimation>, String>>>>>,
    textures: Mutex<HashMap<(String, bool), Arc<OnceLock<Option<Arc<TextureData>>>>>>,
}

impl ModelLoadShared {
    fn new(shaders: Arc<BTreeMap<String, Shader>>) -> Self {
        Self {
            shaders,
            glas: Mutex::new(HashMap::new()),
            textures: Mutex::new(HashMap::new()),
        }
    }
}

struct WorkerModelSource<'a> {
    vfs: &'a mut AssetSearchPath,
    shared: &'a ModelLoadShared,
}

impl ModelSource for WorkerModelSource<'_> {
    fn read_model(&mut self, qpath: &str) -> Result<Vec<u8>, String> {
        Ok(self
            .vfs
            .read(qpath, MAX_GLM_BYTES)
            .map_err(|error| format!("{qpath}: {error}"))?
            .ok_or_else(|| format!("missing {qpath}"))?
            .bytes)
    }

    fn read_optional(&mut self, qpath: &str, max_bytes: usize) -> Result<Option<Vec<u8>>, String> {
        self.vfs
            .read(qpath, max_bytes)
            .map_err(|error| format!("{qpath}: {error}"))
            .map(|asset| asset.map(|asset| asset.bytes))
    }

    fn load_gla(&mut self, qpath: &str) -> Result<Arc<GlaAnimation>, String> {
        let cell = {
            let mut glas = self.shared.glas.lock().map_err(|_| "GLA cache poisoned".to_owned())?;
            Arc::clone(glas.entry(qpath.to_ascii_lowercase()).or_default())
        };
        let vfs = &mut *self.vfs;
        cell.get_or_init(|| {
            if qpath.eq_ignore_ascii_case(OPENJK_DEFAULT_GLA_NAME) {
                return openjk_default_gla().map(Arc::new);
            }
            let bytes = vfs
                .read(qpath, MAX_GLA_BYTES)
                .map_err(|error| format!("{qpath}: {error}"))?
                .ok_or_else(|| format!("missing {qpath}"))?
                .bytes;
            parse_gla(&bytes).map(Arc::new)
        })
        .clone()
    }

    fn load_skin(&mut self, qpath: &str) -> Result<Vec<jka_assets::skin::SkinSurface>, String> {
        let vfs = &mut *self.vfs;
        load_skin(qpath, |name| {
            vfs.read(name, MAX_SKIN_BYTES)
                .map_err(|error| error.to_string())
                .map(|asset| asset.map(|asset| asset.bytes))
        })
    }

    fn resolve_material(&mut self, shader_name: &str) -> ResolvedMaterial {
        let shared = self.shared;
        let layers = material_layers(&shared.shaders, shader_name);
        let (mut texture, mut alpha_mode) = self.load_stage(&layers.base);
        let overlay = layers.overlay.as_ref().map(|stage| {
            let (texture, alpha_mode) = self.load_stage(stage);
            SurfaceOverlay { texture, alpha_mode }
        });
        let stages = layers
            .stages
            .iter()
            .map(|&(stage, mode)| {
                let (texture, _) = self.load_stage(&StageMaterial {
                    image: stage.image.as_str(),
                    clamp: stage.clamp,
                    alpha_mode: mode,
                });
                resolved_stage(stage, mode, texture)
            })
            .collect::<Vec<_>>();
        if let Some(first) = stages.first() {
            texture = first.texture.clone();
            alpha_mode = first.alpha_mode;
        }
        ResolvedMaterial {
            texture,
            alpha_mode,
            entity_tint: layers.entity_tint,
            overlay,
            stages,
            two_sided: layers.two_sided,
        }
    }
}

impl WorkerModelSource<'_> {
    fn load_stage(&mut self, stage: &StageMaterial<'_>) -> (Option<Arc<TextureData>>, DynamicModelAlphaMode) {
        let (image_name, clamp, alpha_mode) = (stage.image, stage.clamp, stage.alpha_mode);
        if image_name.eq_ignore_ascii_case("$whiteimage") {
            return (None, alpha_mode);
        }
        let path = image_name.replace('\\', "/").to_ascii_lowercase();
        let cell = {
            let Ok(mut textures) = self.shared.textures.lock() else {
                return (None, alpha_mode);
            };
            Arc::clone(textures.entry((path.clone(), clamp)).or_default())
        };
        let vfs = &mut *self.vfs;
        let texture = cell
            .get_or_init(|| {
                // A scratch `Textures` reuses the exact decode/extension/
                // stock-override logic of the synchronous loader.
                let mut textures = Textures::new();
                let texture = textures
                    .load(vfs, &path, clamp)
                    .map(|index| Arc::new(textures.images.swap_remove(index)));
                if texture.is_none() {
                    if let Some(warning) = textures.warnings.last() {
                        rverbose!(1, "PLAYER TEXTURE WARNING: {warning}");
                    }
                }
                texture
            })
            .clone();
        (texture, alpha_mode)
    }
}

/// id Tech 3's periodic `base + func(phase + time*frequency) * amplitude`
/// (`RB_CalcWaveColorSingle`/`RB_CalcWaveAlphaSingle`), used here as a
/// per-draw approximation of a shell's `deformVertexes wave` bulge (no
/// per-vertex spatial phase, no real noise stream: `Noise` just holds at the
/// wave's base).
fn wave_value(wave: Wave, time: f32) -> f32 {
    let phase = wave.phase + time * wave.frequency;
    let t = phase - phase.floor();
    let raw = match wave.func {
        WaveFunc::Sin => (phase * std::f32::consts::TAU).sin(),
        WaveFunc::Triangle => {
            if t < 0.5 {
                4.0 * t - 1.0
            } else {
                3.0 - 4.0 * t
            }
        }
        WaveFunc::Square => {
            if t < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        WaveFunc::Sawtooth => 2.0 * t - 1.0,
        WaveFunc::InverseSawtooth => 1.0 - 2.0 * t,
        WaveFunc::Noise => 0.0,
    };
    wave.base + raw * wave.amplitude
}

/// One shader stage as a player/Ghoul2 surface draws it.
struct StageMaterial<'a> {
    image: &'a str,
    clamp: bool,
    alpha_mode: DynamicModelAlphaMode,
}

struct MaterialLayers<'a> {
    base: StageMaterial<'a>,
    entity_tint: bool,
    overlay: Option<StageMaterial<'a>>,
    /// Every drawable stage of a blended multi-stage shader, in order.
    stages: Vec<(&'a jka_assets::shader::Stage, DynamicModelAlphaMode)>,
    two_sided: bool,
}

/// The pass mode of one stage of a blended player shader, or `None` when this
/// path cannot draw it. Not drawn: environment/vector coordinates, alpha tests
/// and `GL_DST_COLOR` filters.
fn blended_stage_mode(stage: &jka_assets::shader::Stage, allow_opaque: bool) -> Option<DynamicModelAlphaMode> {
    use crate::fx::draw::FxBlend;
    if stage.image.is_empty()
        || stage.image.starts_with('$')
        || stage.entity_rgb
        || !stage.alpha_test.trim().is_empty()
        || !matches!(stage.tc_gen, TcGen::Base)
    {
        return None;
    }
    match FxBlend::from_blend_func(&stage.blend) {
        FxBlend::Opaque => allow_opaque.then_some(DynamicModelAlphaMode::Opaque),
        FxBlend::Add => Some(DynamicModelAlphaMode::AdditiveOne),
        FxBlend::AddAlpha => Some(DynamicModelAlphaMode::Additive),
        _ => {
            let blend = stage.blend.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase();
            (blend == "gl_src_alpha gl_one_minus_src_alpha" || blend == "blend")
                .then_some(DynamicModelAlphaMode::BlendUnlit)
        }
    }
}

/// Stage list for the glow/sparkle shaders of custom player models: a shader
/// whose primary stage is blended (not masked or a filter) and has at least one
/// additive stage, or whose primary stage is a plain opaque one (ShadowTink's
/// dark `rgbGen const` body, an eye with a `glow` map) with further drawable
/// stages on top. Alpha-tested primaries keep the single-stage path.
fn blended_stages<'a>(
    shader: &'a Shader,
    primary: &jka_assets::shader::Stage,
) -> Vec<(&'a jka_assets::shader::Stage, DynamicModelAlphaMode)> {
    use crate::fx::draw::FxBlend;
    if primary.entity_rgb || !primary.alpha_test.trim().is_empty() {
        return Vec::new();
    }
    let opaque_primary = match FxBlend::from_blend_func(&primary.blend) {
        FxBlend::Opaque => true,
        FxBlend::Modulate | FxBlend::Modulate2x | FxBlend::Darken => return Vec::new(),
        _ => false,
    };
    let stages = shader
        .stages
        .iter()
        .filter_map(|stage| {
            blended_stage_mode(stage, opaque_primary && std::ptr::eq(stage, primary)).map(|mode| (stage, mode))
        })
        .collect::<Vec<_>>();
    if opaque_primary {
        // The opaque base must itself be drawable, else the overlays would
        // replace it, and a lone base stays on the single-stage path.
        let base_drawn = stages.first().is_some_and(|&(stage, _)| std::ptr::eq(stage, primary));
        return if base_drawn && stages.len() > 1 { stages } else { Vec::new() };
    }
    let additive = stages
        .iter()
        .any(|(_, mode)| matches!(mode, DynamicModelAlphaMode::AdditiveOne | DynamicModelAlphaMode::Additive));
    if additive { stages } else { Vec::new() }
}

fn resolved_stage(
    stage: &jka_assets::shader::Stage,
    alpha_mode: DynamicModelAlphaMode,
    texture: Option<Arc<TextureData>>,
) -> ResolvedStage {
    let const_rgb = stage.rgb_gen == RgbGen::Const;
    ResolvedStage {
        texture,
        alpha_mode,
        rgb: if const_rgb { stage.color.unwrap_or([1.0; 3]) } else { [1.0; 3] },
        alpha: if stage.alpha_gen == AlphaGen::Const { stage.alpha.unwrap_or(1.0) } else { 1.0 },
        unlit: const_rgb,
        specular_alpha: stage.alpha_gen == AlphaGen::LightingSpecular,
        tc_mods: stage.tc_mods.clone(),
    }
}

/// Entity frame and viewer for the per-draw terms of a stage.
struct StageFrame {
    axis: [[f32; 3]; 3],
    origin: [f32; 3],
    viewer: Option<[f32; 3]>,
}

impl StageFrame {
    /// q3's `RB_CalcSpecularAlpha` light: a fixed point in the entity's own
    /// frame, here in JKA world space, with the viewer.
    fn specular_points(&self) -> Option<([f32; 3], [f32; 3])> {
        const LIGHT_LOCAL: [f32; 3] = [-960.0, 1980.0, 96.0];
        let viewer = self.viewer?;
        let light = std::array::from_fn(|i| {
            self.origin[i]
                + self.axis[0][i] * LIGHT_LOCAL[0]
                + self.axis[1][i] * LIGHT_LOCAL[1]
                + self.axis[2][i] * LIGHT_LOCAL[2]
        });
        Some((light, viewer))
    }
}

/// `RB_CalcSpecularAlpha` for one CPU-skinned vertex given in render space.
fn specular_alpha(position: [f32; 3], normal: [f32; 3], light: [f32; 3], viewer: [f32; 3]) -> f32 {
    // Render space is [x, z, -y] of JKA space.
    let to_jka = |v: [f32; 3]| Vec3::new(v[0], -v[2], v[1]);
    let position = to_jka(position);
    let normal = to_jka(normal).normalize_or_zero();
    let light_dir = (Vec3::from_array(light) - position).normalize_or_zero();
    let reflected = normal * (2.0 * normal.dot(light_dir)) - light_dir;
    let to_viewer = (Vec3::from_array(viewer) - position).normalize_or_zero();
    let l = reflected.dot(to_viewer);
    if l < 0.0 {
        0.0
    } else {
        (l * l * l * l).min(1.0)
    }
}

/// `uv * xy + zw` for a stage's `tcMod scale`/`scroll` chain at `seconds`
/// (scroll wraps like the engine's, so the offset never loses precision).
fn stage_uv_xform(mods: &[TcMod], seconds: f32) -> [f32; 4] {
    let (mut scale, mut offset) = ([1.0_f32; 2], [0.0_f32; 2]);
    for tc_mod in mods {
        match *tc_mod {
            TcMod::Scale(x, y) => {
                scale = [scale[0] * x, scale[1] * y];
                offset = [offset[0] * x, offset[1] * y];
            }
            TcMod::Scroll(x, y) => {
                let (sx, sy) = (x * seconds, y * seconds);
                offset = [offset[0] + sx - sx.floor(), offset[1] + sy - sy.floor()];
            }
            _ => {}
        }
    }
    [scale[0], scale[1], offset[0], offset[1]]
}

fn stage_material(stage: &jka_assets::shader::Stage) -> StageMaterial<'_> {
    let alpha_mode = if !stage.alpha_test.trim().is_empty() {
        DynamicModelAlphaMode::Mask
    } else {
        crate::fx::draw::FxBlend::from_blend_func(&stage.blend).custom_shader_alpha_mode()
    };
    StageMaterial { image: stage.image.as_str(), clamp: stage.clamp, alpha_mode }
}

/// The primary stage of a JKA shader decides the image, clamping and alpha
/// mode of a player/Ghoul2 surface. A shader whose primary stage is
/// `rgbGen lightingDiffuseEntity` (the `char_color_*` tint) also carries the
/// alpha-blended stage that redraws the untinted texture over it.
fn material_layers<'a>(shaders: &'a BTreeMap<String, Shader>, shader_name: &'a str) -> MaterialLayers<'a> {
    let unresolved = || MaterialLayers {
        base: StageMaterial { image: shader_name, clamp: false, alpha_mode: DynamicModelAlphaMode::Opaque },
        entity_tint: false,
        overlay: None,
        stages: Vec::new(),
        two_sided: false,
    };
    let Some(shader) = jka_assets::shader::find_shader(shaders, shader_name) else {
        return unresolved();
    };
    let Some(primary) = shader.primary() else {
        return unresolved();
    };
    let overlay = primary
        .entity_rgb
        .then(|| {
            shader
                .stages
                .iter()
                .skip_while(|stage| !std::ptr::eq(*stage, primary))
                .skip(1)
                .find(|stage| {
                    !stage.image.is_empty()
                        && !stage.image.starts_with('$')
                        && !stage.entity_rgb
                        && stage.alpha_test.trim().is_empty()
                        && crate::fx::draw::FxBlend::from_blend_func(&stage.blend) == crate::fx::draw::FxBlend::Alpha
                })
                .map(|stage| StageMaterial {
                    image: stage.image.as_str(),
                    clamp: stage.clamp,
                    // rgbGen lightingDiffuse: a lit, alpha-blended layer.
                    alpha_mode: DynamicModelAlphaMode::Blend,
                })
        })
        .flatten();
    let stages = blended_stages(shader, primary);
    let base = stage_material(primary);
    // Pipelines always cull back faces; only a translucent `cull twosided`
    // surface (wings) shows its back side, so only it pays for the extra triangles.
    let two_sided = matches!(shader.cull.to_ascii_lowercase().as_str(), "none" | "twosided" | "disable")
        && !matches!(
            stages.first().map_or(base.alpha_mode, |&(_, mode)| mode),
            DynamicModelAlphaMode::Opaque | DynamicModelAlphaMode::Mask | DynamicModelAlphaMode::MaskBlend
        );
    MaterialLayers { base, entity_tint: primary.entity_rgb, overlay, stages, two_sided }
}

fn player_jiggle_qpath(model_qpath: &str) -> String {
    model_qpath
        .rsplit_once('.')
        .map_or_else(|| format!("{model_qpath}.jiggle"), |(base, _)| format!("{base}.jiggle"))
}

/// OpenJK `CG_RegisterClientModelname`: GLM + GLA + skin -> drawable surfaces.
fn build_player_model(
    source: &mut dyn ModelSource,
    info: &crate::cgame::ClientInfo,
    key: String,
) -> Result<PlayerModelAsset, String> {
    let model_qpath = info.model_qpath();
    let model_bytes = source.read_model(&model_qpath)?;
    let glm = Arc::new(parse_glm(&model_bytes)?);

    let gla_qpath = if glm.anim_name.to_ascii_lowercase().ends_with(".gla") {
        glm.anim_name.clone()
    } else {
        format!("{}.gla", glm.anim_name)
    };
    let gla = source.load_gla(&gla_qpath)?;
    // OpenJK mdx_format.h: mdxmHeader_t::numBones exists for an
    // in-game version/safety check to ensure the mesh does not reference
    // MORE bones than its GLA provides. Equality is not required; weapon
    // and saber meshes commonly use only a subset of a shared skeleton.
    if glm.num_bones > gla.skeleton.len() {
        return Err(format!(
            "GLM references {} bones but GLA only provides {}",
            glm.num_bones,
            gla.skeleton.len()
        ));
    }

    let jiggle_qpath = player_jiggle_qpath(&model_qpath);
    let explicit_jiggle = match source.read_optional(&jiggle_qpath, MAX_JIGGLE_BYTES) {
        Ok(Some(bytes)) => match String::from_utf8(bytes)
            .map_err(|error| format!("not UTF-8: {error}"))
            .and_then(|text| JiggleProfile::parse(&text, &glm, &gla))
        {
            Ok(profile) => {
                devprintln!(
                    2,
                    "PLAYER JIGGLE: {jiggle_qpath} override loaded regions={}",
                    profile.region_count(),
                );
                Some(profile)
            }
            Err(error) => {
                rverbose!(
                    1,
                    "PLAYER JIGGLE WARNING: {jiggle_qpath}: {error}; trying _humanoid auto profile"
                );
                None
            }
        },
        Ok(None) => None,
        Err(error) => {
            rverbose!(1, "PLAYER JIGGLE WARNING: {error}; trying _humanoid auto profile");
            None
        }
    };
    let jiggle = match explicit_jiggle {
        Some(profile) => Some(Arc::new(profile)),
        None => match JiggleProfile::auto_humanoid(&glm, &gla) {
            Ok(Some(profile)) => {
                devprintln!(
                    2,
                    "PLAYER JIGGLE: {model_qpath} auto regions={} (model.jiggle override supported)",
                    profile.region_count(),
                );
                Some(Arc::new(profile))
            }
            Ok(None) => None,
            Err(error) => {
                rverbose!(1, "PLAYER JIGGLE AUTO WARNING: {model_qpath}: {error}");
                None
            }
        },
    };

    let skin_qpath = info.skin_qpath();
    let skin = match source.load_skin(&skin_qpath) {
        Ok(skin) => skin,
        Err(primary_error) => {
            let fallback = info.default_skin_qpath();
            rverbose!(
                1,
                "PLAYER SKIN FALLBACK: {skin_qpath}: {primary_error}; trying {fallback}"
            );
            source.load_skin(&fallback)?
        }
    };
    let skin_map = skin
        .into_iter()
        .map(|surface| (surface.name.to_ascii_lowercase(), surface.shader))
        .collect::<HashMap<_, _>>();

    let lod = glm
        .lods
        .first()
        .ok_or_else(|| format!("{model_qpath} has no GLM LODs"))?;
    let mut surfaces = Vec::with_capacity(lod.surfaces.len());
    for surface in &lod.surfaces {
        let hierarchy = glm
            .hierarchy
            .get(surface.surface_index)
            .ok_or_else(|| format!("{model_qpath}: missing hierarchy for surface {}", surface.surface_index))?;
        let skin_shader = skin_map
            .get(&hierarchy.name.to_ascii_lowercase())
            .map(String::as_str);
        let skin_off = skin_shader.is_some_and(|shader| shader.eq_ignore_ascii_case("*off"));
        const G2SURFACEFLAG_OFF: u32 = 0x0000_0002;
        let default_visible = !skin_off && hierarchy.flags & G2SURFACEFLAG_OFF == 0;
        // OpenJK keeps `*off` surfaces in the Ghoul2 instance and only toggles
        // their runtime flags. Keep a drawable material resident for those
        // dormant surfaces so stump/limb caps can be enabled without I/O.
        let shader_name = if skin_off {
            hierarchy.shader.as_str()
        } else {
            skin_shader.unwrap_or(hierarchy.shader.as_str())
        };
        if shader_name.is_empty() || shader_name.eq_ignore_ascii_case("*off") {
            continue;
        }
        let material = source.resolve_material(shader_name);
        let two_sided = material.two_sided;
        surfaces.push(PlayerSurfaceAsset {
            surface_index: surface.surface_index,
            default_visible,
            surface_name: hierarchy.name.clone(),
            texture: material.texture,
            alpha_mode: material.alpha_mode,
            entity_tint: material.entity_tint,
            overlay: material.overlay,
            stages: material.stages,
            fallback_gray: false,
            gpu_meshes: build_lod_gpu_meshes(
                &model_qpath,
                &glm,
                surface.surface_index,
                two_sided,
                jiggle.as_deref(),
            )?,
        });
    }

    devprintln!(
        2,
        "PLAYER MODEL: {} skin={} surfaces={} gla={} bones={}",
        model_qpath,
        skin_qpath,
        surfaces.len(),
        gla_qpath,
        gla.skeleton.len(),
    );

    Ok(PlayerModelAsset {
        key,
        glm,
        gla,
        surfaces,
        jiggle,
    })
}

fn build_static_glm(
    source: &mut dyn ModelSource,
    model_qpath: &str,
    custom_skin: Option<&str>,
    label: &str,
    preview_fallback: bool,
) -> Result<SaberModelAsset, String> {
    let model_bytes = source.read_model(model_qpath)?;
    let glm = Arc::new(parse_glm(&model_bytes)?);
    let gla_qpath = if glm.anim_name.to_ascii_lowercase().ends_with(".gla") {
        glm.anim_name.clone()
    } else {
        format!("{}.gla", glm.anim_name)
    };
    let gla = source.load_gla(&gla_qpath)?;
    // Match OpenJK's MDXM registration rule: the mesh may use fewer
    // bones than the referenced GLA, it just may not reference beyond it.
    if glm.num_bones > gla.skeleton.len() {
        return Err(format!(
            "GLM references {} bones but GLA only provides {}",
            glm.num_bones,
            gla.skeleton.len()
        ));
    }

    let skin_map = if let Some(skin_qpath) = custom_skin {
        match source.load_skin(skin_qpath) {
            Ok(skin) => skin
                .into_iter()
                .map(|surface| (surface.name.to_ascii_lowercase(), surface.shader))
                .collect::<HashMap<_, _>>(),
            Err(error) if preview_fallback => {
                rverbose!(
                    1,
                    "ASSET VIEWER GLM SKIN FALLBACK: model={} skin={} error={}; using embedded shaders/gray fallback",
                    model_qpath, skin_qpath, error,
                );
                HashMap::new()
            }
            Err(error) => return Err(error),
        }
    } else {
        HashMap::new()
    };
    let lod = glm
        .lods
        .first()
        .ok_or_else(|| format!("{model_qpath} has no GLM LODs"))?;
    let mut surfaces = Vec::with_capacity(lod.surfaces.len());
    for surface in &lod.surfaces {
        let hierarchy = glm
            .hierarchy
            .get(surface.surface_index)
            .ok_or_else(|| format!("{model_qpath}: missing hierarchy for surface {}", surface.surface_index))?;
        // Ghoul2 tag surfaces are attachment metadata, not visible hilt geometry.
        if hierarchy.name.starts_with('*') {
            continue;
        }
        let shader_name = skin_map
            .get(&hierarchy.name.to_ascii_lowercase())
            .map(String::as_str)
            .unwrap_or(hierarchy.shader.as_str());
        if shader_name.eq_ignore_ascii_case("*off") {
            continue;
        }
        let missing_shader = shader_name.is_empty();
        let shader_name = if missing_shader {
            if preview_fallback {
                // Player/NPC GLMs often have no embedded shader refs and
                // normally rely entirely on model_default.skin. If that
                // skin is absent/incomplete, keep the geometry inspectable
                // instead of silently producing zero draw surfaces.
                "$whiteimage"
            } else {
                continue;
            }
        } else {
            shader_name
        };
        let material = source.resolve_material(shader_name);
        let fallback_gray = preview_fallback
            && (missing_shader
                || (!shader_name.eq_ignore_ascii_case("$whiteimage") && material.texture.is_none()));
        surfaces.push(PlayerSurfaceAsset {
            surface_index: surface.surface_index,
            default_visible: true,
            surface_name: hierarchy.name.clone(),
            texture: material.texture,
            alpha_mode: material.alpha_mode,
            entity_tint: material.entity_tint,
            overlay: material.overlay,
            stages: material.stages,
            fallback_gray,
            gpu_meshes: build_lod_gpu_meshes(&model_qpath, &glm, surface.surface_index, false, None)?,
        });
    }
    devprintln!(
        2,
        "GHOUL2 STATIC MODEL REGISTERED: name={} model={} skin={} surfaces={} glmBones={} glaBones={} gla={}",
        label,
        model_qpath,
        custom_skin.unwrap_or("<default>"),
        surfaces.len(),
        glm.num_bones,
        gla.skeleton.len(),
        gla_qpath,
    );
    Ok(SaberModelAsset { glm, gla, surfaces })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImpulseRagdollSource {
    Weapon,
    Explosion,
    Force,
}

#[derive(Debug, Clone, Copy)]
struct PendingImpulseRagdoll {
    source: ImpulseRagdollSource,
    expires_at: i32,
    /// World-space point the impulse should move away from (weapon/explosion),
    /// or toward for Force Pull when `toward_origin` is true.
    origin: [f32; 3],
    toward_origin: bool,
}

#[derive(Debug, Clone, Copy)]
struct ActiveImpulseRagdoll {
    source: ImpulseRagdollSource,
    expires_at: i32,
    /// Pre-impact trajectory velocity. Free limbs inherit this while the
    /// thoracic kinematic anchor follows the new authoritative trajectory.
    seed_velocity: [f32; 3],
}

#[derive(Debug, Clone, Copy)]
struct ExplosionImpulsePulse {
    origin: [f32; 3],
    expires_at: i32,
}

const IMPULSE_RAGDOLL_MS: i32 = 600;
const IMPULSE_CANDIDATE_MS: i32 = 250;
const IMPULSE_DELTA_MIN: f32 = 48.0;
const EXPLOSION_CANDIDATE_RADIUS: f32 = 512.0;
const FORCE_CANDIDATE_RADIUS: f32 = 1024.0;
const ANIM_TOGGLEBIT: i32 = 2048;

/// Asset/runtime state corresponding to the player-model subset of OpenJK cgame.
/// Models and textures are cached by qpath; animation state is kept per entity,
/// matching the persistent `centity_t` ownership in the original client.
pub struct PlayerPresenter {
    /// This frame's force-power FX requests (drained by the caller).
    fx_requests: Vec<PlayerFxRequest>,
    /// `animevents.cfg` of the stock humanoid skeleton: where footsteps fall in
    /// each animation.
    anim_events: jka_assets::animevents::AnimEvents,
    /// Which `cg_footsteps` stages are on; nothing is traced while all are off.
    footstep_stages: FootstepStages,
    /// This frame's footfalls that found ground (drained by the caller).
    footsteps: Vec<FootstepImpact>,
    /// `Q_irand(0, 99)` for footsteps that only play some of the time.
    footstep_rng: u32,
    /// Snapshot time of the current presentation pass: the clock of shader
    /// `tcMod scroll` on player stages.
    stage_time_ms: i32,
    /// Viewer position (JKA space) for `alphaGen lightingSpecular` stages.
    stage_view_position: Option<[f32; 3]>,
    perf: Ghoul2PerfStats,
    /// Viewer (cg.snap->ps) client and duel state for shells like the duel bubble.
    viewer_client: i32,
    viewer_dueling: bool,
    /// The viewer's `*l_leg_foot` / `*r_leg_foot` in model space from its last
    /// posed frame: `pmove_t::ghoul2` for prediction's leg-dangle slope anims.
    viewer_foot_bolts: Option<[[f32; 3]; 2]>,
    /// What the viewer's model was told to play and the lerp frames it is on, for
    /// the `hitchmark` recorder.
    viewer_anim_debug: Option<ViewerAnimDebug>,
    /// jaPRO client options (`cg_stylePlayer`, cosmetics switches).
    japro: crate::japro_cg::JaproCgame,
    /// The viewer's state as `CG_Player` reads it to cull and restyle others.
    viewer_style: crate::japro_cg::StyleViewer,
    /// Appearance of the player currently being presented (set per entity).
    look: crate::japro_cg::Appearance,
    /// jaPRO `jcinfo2` feature bits used by CG_Player presentation. In
    /// particular WTTRIBES swaps the stock Boba jet nozzles for the Tribes
    /// thrust presentation.
    japro_cinfo2: i32,
    /// TaystJK cg_saberTeamColors (archive, default 1). Skin forcing is unconditional
    /// in ordinary team games; this only controls saber normalization.
    saber_team_colors: bool,
    /// TaystJK cg_saberStaffMultiColor (archive, default 0).
    saber_staff_multi_color: bool,
    /// Glow brightness (0..1) of the private-duel shell while the viewer duels.
    duel_shell_gray: Option<f32>,
    /// Cosmetic MD3s bolted to players this frame (drained by the caller).
    cosmetic_draws: Vec<CosmeticDraw>,
    /// `cp_cosmetics` of this client in a local (solo) game, which has no `c5`.
    local_cosmetics: u32,
    /// `cp_cosmetics` this client last sent a jaPRO server, to spot the server
    /// relaying (`c5`) something else, e.g. after stripping locked cosmetics.
    sent_cosmetics: u32,
    cosmetic_mismatch_logged: Option<(u32, u32)>,
    /// `cent->teamPowerEffectTime` / `teamPowerType`: (expiry time, type) per
    /// client. Types: 0 regen, 1 heal, 2 drain, 3 absorb hit.
    team_power: HashMap<u16, (i32, u8)>,
    /// `cent->bodyFadeTime` start per corpse (EV_BODYFADE): the body turns
    /// transparent and is gone after about 3.2 s.
    body_fade: HashMap<u16, i32>,
    /// `cg_ghoul2Marks`: burn marks kept per model (0 disables them).
    gore_limit: usize,
    gore: Vec<super::player_gore::Gore>,
    gore_next_id: u32,
    /// GPU-skinned body surfaces of each player's last presented frame.
    last_gpu: HashMap<u16, Vec<super::player_gore::GpuSnap>>,
    /// The last presented pose of each vehicle: muzzle bolts for EV_VEH_FIRE.
    vehicle_snaps: HashMap<u16, VehicleSnap>,
    assets: AssetSearchPath,
    pbr: bool,
    shaders: Arc<BTreeMap<String, Shader>>,
    textures: Textures,
    texture_arcs: HashMap<usize, Arc<TextureData>>,
    animations: AnimationSet,
    saber_scales: SaberAnimationScales,
    saber_definitions: SaberDefinitions,
    vehicle_definitions: VehicleDefinitions,
    models: HashMap<String, Arc<PlayerModelAsset>>,
    saber_models: HashMap<String, Arc<SaberModelAsset>>,
    /// Asynchronous registration (see `asset_jobs`). These own the pending and
    /// terminal states of runtime cache misses; `models`/`saber_models` above
    /// keep the synchronous path's results (previews, `cg_asyncAssets 0`).
    async_models: AssetRegistry<PlayerModelAsset>,
    async_static_models: AssetRegistry<SaberModelAsset>,
    asset_source: Arc<AssetSource>,
    load_shared: Arc<ModelLoadShared>,
    /// Per-presenter opt-out of asynchronous registration (tests/tools that
    /// need results on the very next call). `cg_asyncAssets` is the global switch.
    async_loading: bool,
    entities: HashMap<u16, EntityPlayerState>,
    /// Presentation-only entities (race ghosts, future auxiliary players) that
    /// are not part of the authoritative snapshot list but still own persistent
    /// Ghoul2 animation/model state across frames.
    external_live_entities: HashSet<u16>,
    /// Lightweight post-Ghoul2 soft-tissue secondary motion. Stock `_humanoid`
    /// models auto-compile chest/glute masks; `model.jiggle` remains an override.
    jiggle: JiggleSystem,
    /// Experimental presentation-only cloth on authored cape/cloak/robe surfaces.
    cloth: ClothSystem,
    cloth_weather_wind: Option<crate::ocean::OceanWind>,
    cloth_body_templates: HashMap<(String, usize, Vec<usize>), Vec<(usize, ClothCapsule)>>,
    /// Broadsword-compatible presentation seam backed by Rapier.
    ragdolls: RagdollWorld,
    /// Vanilla entityState does not transmit forceGripEntityNum. Preserve the
    /// last target per active gripper and reacquire with OpenJK's 256-unit
    /// view trace when needed. The local victim uses the exact playerState
    /// forceGripCripple flag supplied by App.
    force_grip_targets: HashMap<u16, u16>,
    force_gripped_entities: HashSet<u16>,
    /// Short-lived, event-correlated candidates. Remote entityState does not
    /// transmit PMF_TIME_KNOCKBACK, so we confirm these against an
    /// authoritative trajectory-velocity discontinuity before ragdolling.
    pending_impulse_ragdolls: HashMap<u16, PendingImpulseRagdoll>,
    active_impulse_ragdolls: HashMap<u16, ActiveImpulseRagdoll>,
    explosion_impulse_pulses: Vec<ExplosionImpulsePulse>,
    previous_impulse_velocity: HashMap<u16, [f32; 3]>,
    /// Raw torsoAnim (toggle bit preserved) lets repeated Force Push/Pull
    /// gestures be detected even when the animation index itself is unchanged.
    force_gesture_anim: HashMap<u16, i32>,
    thrown_sabers: HashMap<u16, SaberThrowState>,
    /// OpenJK CG_BodyQueueCopy state delivered by reliable `ircg`.
    body_queue_copies: HashMap<u16, BodyQueueCopyState>,
    /// Runtime surface state from authoritative server G2_MODEL_PART entities.
    dismembered: HashMap<u16, HashSet<DismemberPart>>,
    /// Last fully evaluated player/body pose, used to clone the severed Ghoul2
    /// subset at the exact visual frame the server part first appears.
    dismember_source_snaps: HashMap<u16, DismemberSourceSnap>,
    detached_limb_visuals: HashMap<u16, DetachedLimbVisual>,
    /// OpenJK clientInfo_t saber blade current/desired lengths.
    saber_blade_lengths: HashMap<SaberBladeLengthKey, SaberBladeLengthState>,
    failed_models: HashMap<String, String>,
    player_diagnostics: HashMap<u16, String>,
    failed_saber_models: HashSet<String>,
    reported_saber_warnings: HashSet<String>,
    reported_saber_diagnostics: HashSet<String>,
    reported_vehicle_fallbacks: HashSet<String>,
    logged_first_draw: bool,
    skinning_mode: Ghoul2SkinningMode,
    early_frustum_cull: bool,
    /// When true, camera-culled Ghoul2 models may still be submitted as
    /// shadow-only RT casters. Normal RT-off presentation is unchanged.
    rt_shadow_casters_enabled: bool,
    /// Map-scoped collision clone used only by OpenJK cg_shadows 1. Like the
    /// original CM_Trace call this traces static BSP collision, not entities.
    collision_world: Option<CollisionWorld>,
    blob_shadows_enabled: bool,
    blob_shadow_instances: Vec<FxGpuSpriteInstance>,
    blob_shadow_texture: Option<Arc<TextureData>>,
    blob_shadow_alpha_mode: DynamicModelAlphaMode,
    blob_shadow_texture_resolved: bool,
    blob_shadow_asset_warned: bool,
    lod_bias: i32,
    /// `r_lodscale` (OpenJK default 5).
    lod_scale: f32,
    /// `r_ghoul2animsmooth`: jaPRO `CBoneCache::SmoothLow` factor, applied to
    /// the final composed bone pose before it reaches GPU skinning,
    /// attachments, and drawing. Defaults to jaPRO's own default of `0.3`.
    /// Only active strictly inside `(0, 1)`, matching jaPRO's
    /// `val>0.0f&&val<1.0f` gate: `0` or `>= 1` disables it, so `1.0` is
    /// "off", not "maximum".
    ghoul2_anim_smooth: f32,
    /// TaystJK CG_MapTorsoToWeaponFrame's process-wide busy-holster state.
    view_weapon_frame: i32,
    view_weapon_frame_time: i32,
    skinning_pool: rayon::ThreadPool,
}

fn resolve_vehicle_model_request(
    definitions: &VehicleDefinitions,
    requested: &str,
) -> Result<(String, Option<String>, String), String> {
    let (model_request, appended_skin) = match requested.rsplit_once('*') {
        Some((model, skin)) if !skin.is_empty() => (model, Some(skin)),
        Some((model, _)) => (model, None),
        None => (requested, None),
    };

    let (model_value, authored_skin, label) = if let Some(vehicle_name) = model_request.strip_prefix('$') {
        let definition = definitions.get(vehicle_name).ok_or_else(|| {
            format!("vehicle definition {model_request} was not found in ext_data/vehicles/*.veh")
        })?;
        (
            definition.model.as_str(),
            definition.skin.as_deref(),
            format!("{model_request} ({})", definition.model),
        )
    } else {
        (model_request, None, model_request.to_owned())
    };

    let normalized = model_value.replace('\\', "/");
    let glm = if normalized.to_ascii_lowercase().ends_with(".glm") {
        normalized
    } else if normalized.to_ascii_lowercase().starts_with("models/") {
        format!("{}/model.glm", normalized.trim_end_matches('/'))
    } else {
        format!("models/players/{}/model.glm", normalized.trim_matches('/'))
    };

    // OpenJK registers a vehicle's authored skin, or model_default.skin when
    // the .veh leaves `skin` blank. Do not treat the vehicle like a saber hilt
    // and rely on embedded surface shaders here.
    let skin_name = appended_skin.or(authored_skin).unwrap_or("default");
    let lower_glm = glm.to_ascii_lowercase();
    let skin = lower_glm.strip_suffix("model.glm").map(|_| {
        let prefix = &glm[..glm.len() - "model.glm".len()];
        format!("{prefix}model_{skin_name}.skin")
    });
    Ok((glm, skin, label))
}

impl PlayerPresenter {
    /// Reset only per-entity/time-dependent presentation state for demo seeking.
    /// Asset/model caches stay hot so scrubbing does not turn into a reload path.
    pub fn reset_for_seek(&mut self) {
        self.fx_requests.clear();
        self.footsteps.clear();
        self.entities.clear();
        self.jiggle.clear();
        self.team_power.clear();
        self.body_fade.clear();
        self.gore.clear();
        self.last_gpu.clear();
        self.vehicle_snaps.clear();
        self.force_grip_targets.clear();
        self.force_gripped_entities.clear();
        self.pending_impulse_ragdolls.clear();
        self.active_impulse_ragdolls.clear();
        self.explosion_impulse_pulses.clear();
        self.previous_impulse_velocity.clear();
        self.force_gesture_anim.clear();
        self.thrown_sabers.clear();
        self.body_queue_copies.clear();
        self.dismembered.clear();
        self.dismember_source_snaps.clear();
        self.detached_limb_visuals.clear();
        self.saber_blade_lengths.clear();
        self.blob_shadow_instances.clear();
        self.cloth.reset();
        self.ragdolls.reset_dynamic_for_seek();
        self.view_weapon_frame = 0;
        self.view_weapon_frame_time = 0;
    }

    pub fn vehicle_definition_for_model_request(&self, requested: &str) -> Option<&VehicleDefinition> {
        let model_request = requested
            .rsplit_once('*')
            .map_or(requested, |(model, _)| model);
        let vehicle_name = model_request.strip_prefix('$')?;
        self.vehicle_definitions.get(vehicle_name)
    }

    pub fn new(mut assets: AssetSearchPath, pbr: bool) -> Result<Self, String> {
        let animations = load_humanoid_animations(&mut assets)?;
        let anim_events = jka_assets::animevents::load_humanoid_animation_events(&mut assets, &animations)
            .unwrap_or_else(|error| {
                println!("PLAYER ASSETS: no animation events, so no footsteps: {error}");
                Default::default()
            });
        let saber_scales = load_saber_animation_scales(&mut assets)?;
        let saber_definitions = load_saber_definitions(&mut assets)?;
        let vehicle_definitions = load_vehicle_definitions(&mut assets)?;
        let mut shader_warnings = Vec::new();
        let (shaders, diagnostics) =
            materials::shader_library(&mut assets, &mut shader_warnings, pbr)?;
        devprintln!(
            1,
            "PLAYER ASSETS: humanoid animations={} saberDefs={} vehicleDefs={} shaderDefs={} mtrDefs={}",
            animations.animations.len(),
            saber_definitions.len(),
            vehicle_definitions.len(),
            diagnostics.shader_definitions,
            diagnostics.mtr_definitions,
        );
        for warning in shader_warnings {
            rverbose!(1, "PLAYER MATERIAL WARNING: {warning}");
        }
        let asset_source = AssetSource::from_search_path(&assets);
        let shaders = Arc::new(shaders);
        let load_shared = Arc::new(ModelLoadShared::new(Arc::clone(&shaders)));
        let worker_count = std::thread::available_parallelism()
            .map_or(4, |count| count.get())
            .saturating_sub(2)
            .clamp(1, 8);
        let skinning_pool = rayon::ThreadPoolBuilder::new()
            .num_threads(worker_count)
            .thread_name(|index| format!("jka-g2-skin-{index}"))
            .build()
            .map_err(|error| format!("could not start Ghoul2 skinning workers: {error}"))?;
        devprintln!(1, "Ghoul2 skinning worker pool: {worker_count} thread(s)");
        let mut presenter = Self {
            assets,
            pbr,
            shaders,
            textures: Textures::new(),
            texture_arcs: HashMap::new(),
            animations,
            saber_scales,
            saber_definitions,
            vehicle_definitions,
            models: HashMap::new(),
            saber_models: HashMap::new(),
            async_models: AssetRegistry::new("player model"),
            async_static_models: AssetRegistry::new("ghoul2 model"),
            asset_source,
            load_shared,
            async_loading: true,
            fx_requests: Vec::new(),
            anim_events,
            footstep_stages: FootstepStages::from_level(footsteps::FOOTSTEPS_MARKS),
            footsteps: Vec::new(),
            footstep_rng: 0x2545_f491,
            stage_time_ms: 0,
            stage_view_position: None,
            perf: Ghoul2PerfStats::default(),
            viewer_client: -1,
            viewer_foot_bolts: None,
            viewer_anim_debug: None,
            viewer_dueling: false,
            japro: crate::japro_cg::JaproCgame::default(),
            viewer_style: crate::japro_cg::StyleViewer::default(),
            look: crate::japro_cg::Appearance::NORMAL,
            japro_cinfo2: 0,
            saber_team_colors: true,
            saber_staff_multi_color: false,
            duel_shell_gray: None,
            cosmetic_draws: Vec::new(),
            local_cosmetics: 0,
            sent_cosmetics: 0,
            cosmetic_mismatch_logged: None,
            team_power: HashMap::new(),
            body_fade: HashMap::new(),
            gore_limit: 0,
            gore: Vec::new(),
            gore_next_id: 1,
            last_gpu: HashMap::new(),
            vehicle_snaps: HashMap::new(),
            entities: HashMap::new(),
            external_live_entities: HashSet::new(),
            jiggle: JiggleSystem::default(),
            cloth: ClothSystem::default(),
            cloth_weather_wind: None,
            cloth_body_templates: HashMap::new(),
            ragdolls: RagdollWorld::new(),
            force_grip_targets: HashMap::new(),
            force_gripped_entities: HashSet::new(),
            pending_impulse_ragdolls: HashMap::new(),
            active_impulse_ragdolls: HashMap::new(),
            explosion_impulse_pulses: Vec::new(),
            previous_impulse_velocity: HashMap::new(),
            force_gesture_anim: HashMap::new(),
            thrown_sabers: HashMap::new(),
            body_queue_copies: HashMap::new(),
            dismembered: HashMap::new(),
            dismember_source_snaps: HashMap::new(),
            detached_limb_visuals: HashMap::new(),
            saber_blade_lengths: HashMap::new(),
            failed_models: HashMap::new(),
            player_diagnostics: HashMap::new(),
            failed_saber_models: HashSet::new(),
            reported_saber_warnings: HashSet::new(),
            reported_saber_diagnostics: HashSet::new(),
            reported_vehicle_fallbacks: HashSet::new(),
            logged_first_draw: false,
            skinning_mode: Ghoul2SkinningMode::Gpu,
            early_frustum_cull: true,
            rt_shadow_casters_enabled: false,
            collision_world: None,
            blob_shadows_enabled: false,
            blob_shadow_instances: Vec::new(),
            blob_shadow_texture: None,
            blob_shadow_alpha_mode: DynamicModelAlphaMode::BlendUnlit,
            blob_shadow_texture_resolved: false,
            blob_shadow_asset_warned: false,
            lod_bias: 0,
            lod_scale: crate::fx::LOD_SCALE_DEFAULT,
            ghoul2_anim_smooth: 0.3,
            view_weapon_frame: 0,
            view_weapon_frame_time: 0,
            skinning_pool,
        };
        presenter.prime_default_player_models();
        Ok(presenter)
    }

    /// TaystJK `CG_MapTorsoToWeaponFrame`: sample the already-advanced local
    /// Ghoul2 lower_lumbar animation and map it onto the MD3 hand animation.
    /// The returned `(frame, oldframe, backlerp)` is consumed by the viewmodel
    /// tag interpolation exactly like refEntity_t.
    pub fn view_weapon_frames(
        &mut self,
        entity_num: u16,
        current_time: i32,
        torso_anim: i32,
        force_hand_extend: i32,
    ) -> (usize, usize, f32) {
        const HANDEXTEND_NONE: i32 = 0;

        // WEAPON_FORCE_BUSY_HOLSTER from TaystJK: advance frames 6..10 at
        // 10 ms steps while a hand-extend is active, then retain frame 10 for
        // 100 ms so the weapon cannot snap to idle for one render frame.
        if force_hand_extend != HANDEXTEND_NONE || self.view_weapon_frame_time > current_time {
            if self.view_weapon_frame < 6 {
                self.view_weapon_frame = 6;
                self.view_weapon_frame_time = current_time + 10;
            } else if self.view_weapon_frame_time < current_time && self.view_weapon_frame < 10 {
                self.view_weapon_frame += 1;
                self.view_weapon_frame_time = current_time + 10;
            } else if force_hand_extend != HANDEXTEND_NONE && self.view_weapon_frame == 10 {
                self.view_weapon_frame_time = current_time + 100;
            }
            let frame = self.view_weapon_frame.max(0) as usize;
            return (frame, frame, 0.0);
        }
        self.view_weapon_frame = 0;
        self.view_weapon_frame_time = 0;

        let current_frame = self.entities.get_mut(&entity_num).and_then(|state| {
            let lower_lumbar = Ghoul2Animator::bone_index(&state.model.gla, "lower_lumbar")?;
            state
                .animation
                .animator
                .bone_frame(&state.model.gla, lower_lumbar, current_time)
                .ok()
                .flatten()
        });
        let Some(current_frame) = current_frame else {
            return (0, 0, 0.0);
        };

        let Some(animation) = self.animations.get(torso_anim) else {
            return (0, 0, 0.0);
        };
        let first = i32::from(animation.first_frame);
        let map = |frame: i32| -> Option<usize> {
            let offset = frame - first;
            match AnimationSet::name(torso_anim) {
                Some("TORSO_DROPWEAP1") if (0..5).contains(&offset) => Some((offset + 6) as usize),
                Some("TORSO_RAISEWEAP1") if (0..4).contains(&offset) => Some((offset + 10) as usize),
                Some(
                    "BOTH_ATTACK1"
                    | "BOTH_ATTACK2"
                    | "BOTH_ATTACK3"
                    | "BOTH_ATTACK4"
                    | "BOTH_ATTACK10"
                    | "BOTH_THERMAL_THROW",
                ) if (0..6).contains(&offset) => Some((offset + 1) as usize),
                _ => None,
            }
        };

        let frame = map(current_frame.ceil() as i32);
        let oldframe = map(current_frame.floor() as i32);
        match (frame, oldframe) {
            (None, _) => (0, 0, 0.0),
            (Some(frame), None) => (frame, frame, 0.0),
            (Some(frame), Some(oldframe)) => {
                (frame, oldframe, 1.0 - current_frame.fract())
            }
        }
    }

    /// `cg_footsteps` picks the sound and dust stages; prints follow the
    /// Footprints video setting alone (`prints`).
    pub fn set_footstep_level(&mut self, level: u8, prints: bool) {
        self.footstep_stages = FootstepStages { marks: prints, ..FootstepStages::from_level(level) };
    }

    pub fn footstep_stages(&self) -> FootstepStages {
        self.footstep_stages
    }

    /// Footfalls that found ground since the last call.
    pub fn drain_footsteps(&mut self) -> Vec<FootstepImpact> {
        std::mem::take(&mut self.footsteps)
    }

    /// Force-power effect requests produced since the last call.
    pub fn drain_fx_requests(&mut self) -> Vec<PlayerFxRequest> {
        std::mem::take(&mut self.fx_requests)
    }

    /// Cosmetic (hat/cape) MD3 submissions produced since the last call. The
    /// entity presenter owns MD3 loading, so the app forwards these to it.
    pub fn drain_cosmetic_draws(&mut self) -> Vec<CosmeticDraw> {
        std::mem::take(&mut self.cosmetic_draws)
    }

    /// The viewer's own `cp_cosmetics` in a local game, where there is no server
    /// to relay it back as `c5`.
    pub fn set_local_cosmetics(&mut self, mask: u32) {
        self.local_cosmetics = mask;
    }

    pub fn set_saber_team_colors(&mut self, enabled: bool) {
        self.saber_team_colors = enabled;
    }

    pub fn set_saber_staff_multi_color(&mut self, enabled: bool) {
        self.saber_staff_multi_color = enabled;
    }

    /// The `cp_cosmetics` this client sends a jaPRO server (diagnostics only).
    pub fn set_sent_cosmetics(&mut self, mask: u32) {
        self.sent_cosmetics = mask;
    }

    /// jaPRO client options (`cg_stylePlayer` and friends).
    pub fn set_japro_options(&mut self, options: crate::japro_cg::JaproCgame) {
        self.japro = options;
    }

    pub fn begin_perf_frame(&mut self) {
        self.perf = Ghoul2PerfStats::default();
    }

    /// OpenJK CG_RestoreClientGhoul_f / CG_BodyQueueCopy presentation state.
    pub fn apply_ghoul2_server_command(&mut self, command: Ghoul2ServerCommand) {
        match command {
            Ghoul2ServerCommand::BodyQueueCopy {
                source_client,
                body_entity,
                known_weapon,
                light_side,
            } => {
                // CG_RestoreClientGhoul_f returns immediately when the source
                // has no Ghoul2 instance. `entities` is this presenter's
                // equivalent persistent instance state, so do the same here.
                let Some(source) = self.entities.get(&source_client) else {
                    return;
                };
                let source_model1 = if source.primary_saber_attached {
                    Some(WP_SABER)
                } else {
                    source.attached_weapon
                };
                let source_model2_saber = source.secondary_saber_attached;

                // CG_BodyQueueCopy duplicates the actual source Ghoul2 first,
                // then applies knownWeapon only to model index 1.
                // A reused body-queue slot starts a fresh life.
                self.body_fade.remove(&body_entity);
                if let Some(parts) = self.dismembered.get(&source_client).cloned() {
                    self.dismembered.insert(body_entity, parts);
                } else {
                    self.dismembered.remove(&body_entity);
                }
                // Preserve the already-solved corpse articulation across the
                // live-player -> ET_BODY ownership transfer. OpenJK copies the
                // existing Ghoul2 instance here rather than replaying death.
                self.ragdolls.body_queue_copy(source_client, body_entity);
                self.body_queue_copies.insert(
                    body_entity,
                    BodyQueueCopyState {
                        source_client,
                        _known_weapon: known_weapon,
                        _light_side: light_side,
                        model1_weapon: body_queue_model1_weapon(source_model1, known_weapon),
                        model2_saber: source_model2_saber,
                    },
                );

                // The command handler resets the live centity tracking fields
                // after the body has been copied. It does not remove the source
                // model slots themselves.
                if let Some(source) = self.entities.get_mut(&source_client) {
                    source.cent_weapon = 0;
                    source.ghoul2_weapon = None;
                }
                self.thrown_sabers.remove(&source_client);
            }
            Ghoul2ServerCommand::RestoreClient { source_client } => {
                // Same early return as CG_RestoreClientGhoul_f.
                let Some(source) = self.entities.get_mut(&source_client) else {
                    return;
                };
                source.cent_weapon = 0;
                source.ghoul2_weapon = None;
                self.thrown_sabers.remove(&source_client);
                // CG_ReattachLimb restores the live player's original Ghoul2
                // surface flags after the body queue copy. The corpse retains
                // the cloned cut state above.
                self.dismembered.remove(&source_client);
            }
            Ghoul2ServerCommand::KillEntity { entity } => {
                // CG_KillGhoul2_f / CG_KillCEntityG2: drop the persistent
                // presentation instance. If a later snapshot still references
                // this slot it will be recreated from its current entity state.
                self.entities.remove(&entity);
                self.ragdolls.remove_entity(entity);
                self.body_queue_copies.remove(&entity);
                self.body_fade.remove(&entity);
                self.dismembered.remove(&entity);
                self.dismember_source_snaps.remove(&entity);
                self.detached_limb_visuals.remove(&entity);
                self.thrown_sabers.remove(&entity);
                self.team_power.remove(&entity);
                self.force_grip_targets.remove(&entity);
                self.force_grip_targets.retain(|_, target| *target != entity);
                self.force_gripped_entities.remove(&entity);
                self.pending_impulse_ragdolls.remove(&entity);
                self.active_impulse_ragdolls.remove(&entity);
                self.previous_impulse_velocity.remove(&entity);
                self.force_gesture_anim.remove(&entity);
                self.last_gpu.remove(&entity);
                self.vehicle_snaps.remove(&entity);
                self.player_diagnostics.remove(&entity);
            }
        }
    }

    /// OpenJK EV_DESTROY_WEAPON_MODEL removes Ghoul2 model index 1 only.
    pub fn apply_entity_event(&mut self, event: &PresentationEvent) {
        self.observe_impulse_event(event);
        let state = &event.state;
        let until = event.server_time.saturating_add(1000);
        match event.event {
            EntityEvent::EV_FORCE_DRAINED => {
                if let Ok(owner) = u16::try_from(state.field_i32("owner").unwrap_or(-1)) {
                    self.team_power.insert(owner, (until, 2));
                }
                return;
            }
            EntityEvent::EV_TEAM_POWER => {
                // CG_InClientBitflags: clients 0-15 in trickedentindex, 16-31 in trickedentindex2.
                let low = state.field_i32("trickedentindex").unwrap_or(0);
                let high = state.field_i32("trickedentindex2").unwrap_or(0);
                for client in 0u16..32 {
                    let bits = if client > 15 { high >> (client - 16) } else { low >> client };
                    if bits & 1 != 0 {
                        // eventParm 1 is heal, anything else force regen.
                        self.team_power.insert(client, (until, u8::from(event.parm == 1)));
                    }
                }
                return;
            }
            EntityEvent::EV_VEH_FIRE => {
                self.vehicle_muzzle_fire(event);
                return;
            }
            EntityEvent::EV_GHOUL2_MARK => {
                self.add_gore_mark(event);
                return;
            }
            EntityEvent::EV_BODYFADE => {
                if state.field_i32("eType").unwrap_or(0) == ET_BODY as i32 {
                    self.body_fade.insert(event.entity_num, event.server_time);
                }
                return;
            }
            EntityEvent::EV_PREDEFSOUND if event.parm == 3 => {
                // PDSOUND_ABSORBHIT: the absorbing client flashes its shield.
                if let Ok(client) = u16::try_from(state.field_i32("trickedentindex").unwrap_or(-1)) {
                    if client < 32 {
                        self.team_power.insert(client, (until, 3));
                    }
                }
                return;
            }
            _ => {}
        }
        if event.event != EntityEvent::EV_DESTROY_WEAPON_MODEL {
            return;
        }
        let Ok(target) = u16::try_from(event.parm) else {
            return;
        };
        if let Some(runtime) = self.entities.get_mut(&target) {
            runtime.attached_weapon = None;
            runtime.primary_saber_attached = false;
        }
    }

    fn observe_impulse_event(&mut self, event: &PresentationEvent) {
        const EF_ALT_FIRING: i32 = 1 << 10;
        let state = &event.state;
        let weapon = state.field_i32("weapon").unwrap_or(0);
        let alt = state.field_i32("eFlags").unwrap_or(0) & EF_ALT_FIRING != 0;
        let explosive = impulse_weapon_is_explosive(weapon, alt);

        match event.event {
            EntityEvent::EV_SABER_HIT if self.ragdolls.weapon_impulses_enabled() => {
                if let Ok(target) = u16::try_from(state.field_i32("otherEntityNum").unwrap_or(-1)) {
                    self.queue_impulse_candidate(
                        target,
                        ImpulseRagdollSource::Weapon,
                        event.position,
                        false,
                        event.server_time,
                    );
                }
            }
            EntityEvent::EV_MISSILE_HIT => {
                let source = if explosive {
                    ImpulseRagdollSource::Explosion
                } else {
                    ImpulseRagdollSource::Weapon
                };
                let enabled = match source {
                    ImpulseRagdollSource::Weapon => self.ragdolls.weapon_impulses_enabled(),
                    ImpulseRagdollSource::Explosion => self.ragdolls.explosion_impulses_enabled(),
                    ImpulseRagdollSource::Force => false,
                };
                if enabled {
                    if let Ok(target) = u16::try_from(state.field_i32("otherEntityNum").unwrap_or(-1)) {
                        self.queue_impulse_candidate(target, source, event.position, false, event.server_time);
                    }
                }
                if explosive && self.ragdolls.explosion_impulses_enabled() {
                    self.explosion_impulse_pulses.push(ExplosionImpulsePulse {
                        origin: event.position,
                        expires_at: event.server_time.saturating_add(IMPULSE_CANDIDATE_MS),
                    });
                }
            }
            EntityEvent::EV_MISSILE_MISS | EntityEvent::EV_MISSILE_MISS_METAL
                if explosive && self.ragdolls.explosion_impulses_enabled() =>
            {
                self.explosion_impulse_pulses.push(ExplosionImpulsePulse {
                    origin: event.position,
                    expires_at: event.server_time.saturating_add(IMPULSE_CANDIDATE_MS),
                });
            }
            _ => {}
        }
    }

    fn queue_impulse_candidate(
        &mut self,
        target: u16,
        source: ImpulseRagdollSource,
        origin: [f32; 3],
        toward_origin: bool,
        current_time: i32,
    ) {
        self.pending_impulse_ragdolls.insert(
            target,
            PendingImpulseRagdoll {
                source,
                expires_at: current_time.saturating_add(IMPULSE_CANDIDATE_MS),
                origin,
                toward_origin,
            },
        );
    }

    pub fn set_gore_limit(&mut self, limit: usize) {
        self.gore_limit = limit.min(64);
        if self.gore_limit == 0 {
            self.gore.clear();
            self.last_gpu.clear();
        }
    }

    /// `CG_VehMuzzleFireFX`: the muzzle effect of every muzzle named in the
    /// event's bit mask (`trickedentindex`), at that muzzle's bolt on the vehicle.
    fn vehicle_muzzle_fire(&mut self, event: &PresentationEvent) {
        let state = &event.state;
        let Ok(owner) = u16::try_from(state.field_i32("owner").unwrap_or(-1)) else { return };
        let mask = state.field_i32("trickedentindex").unwrap_or(0);
        let Some(snap) = self.vehicle_snaps.get(&owner).cloned() else { return };
        let Some(definition) = self.vehicle_definitions.get(&snap.vehicle) else { return };
        let mut requests = Vec::new();
        for (index, weapon) in definition.weap_muzzles.iter().enumerate() {
            let Some(weapon) = weapon else { continue };
            if mask & (1 << index) == 0 {
                continue;
            }
            let Some(effect) = self.vehicle_definitions.weapon_muzzle_fx(weapon) else { continue };
            let number = index + 1;
            let bolt = [format!("*muzzle{number}"), format!("*flash{number}")].into_iter().find_map(|tag| {
                model_bolt_matrix_timed(&mut self.perf, &snap.model.glm, &snap.model.gla, &snap.pose, &tag)
                    .ok()
                    .flatten()
            });
            let Some(m) = bolt else { continue };
            let origin = transform_jka_model_point([m[0][3], m[1][3], m[2][3]], snap.axis, snap.origin);
            // NEGATIVE_Y of the bolt matrix is the muzzle direction.
            let dir = transform_jka_model_vector([-m[0][1], -m[1][1], -m[2][1]], snap.axis);
            requests.push(PlayerFxRequest::EffectDir { name: effect.to_owned(), origin, dir });
        }
        self.fx_requests.extend(requests);
    }

    /// `CG_G2MarkEvent`: burn a decal onto the hit player's model.
    fn add_gore_mark(&mut self, event: &PresentationEvent) {
        const WP_BRYAR_PISTOL: i32 = 4;
        const WP_BLASTER: i32 = 5;
        const WP_DISRUPTOR: i32 = 6;
        const WP_BOWCASTER: i32 = 7;
        const WP_REPEATER: i32 = 8;
        const WP_ROCKET_LAUNCHER: i32 = 11;
        const WP_THERMAL: i32 = 12;
        const WP_CONCUSSION: i32 = 15;
        const WP_BRYAR_OLD: i32 = 16;
        const WP_TURRET: i32 = 17;
        if self.gore_limit == 0 {
            return;
        }
        let state = &event.state;
        let Ok(target) = u16::try_from(state.field_i32("otherEntityNum").unwrap_or(-1)) else { return };
        let (size, shader) = match state.field_i32("weapon").unwrap_or(0) {
            WP_BRYAR_PISTOL | WP_CONCUSSION | WP_BRYAR_OLD | WP_BLASTER | WP_DISRUPTOR | WP_BOWCASTER | WP_REPEATER | WP_TURRET => {
                (4.0, "gfx/damage/bodyburnmark1")
            }
            WP_ROCKET_LAUNCHER | WP_THERMAL => (24.0, "gfx/damage/bodybigburnmark1"),
            _ => return,
        };
        let Some(snaps) = self.last_gpu.get(&target) else { return };
        let origin = super::entity_vec3(state, "origin").unwrap_or(event.position);
        let predicted = super::entity_vec3(state, "origin2").unwrap_or(origin);
        let id = self.gore_next_id;
        self.gore_next_id = self.gore_next_id.wrapping_add(1);
        let surfaces = super::player_gore::build_gore(snaps, id, origin, predicted, event.parm != 0, size);
        if surfaces.is_empty() {
            return;
        }
        while self.gore.iter().filter(|gore| gore.entity == target).count() >= self.gore_limit {
            let Some(oldest) = self.gore.iter().position(|gore| gore.entity == target) else { break };
            self.gore.remove(oldest);
        }
        self.gore.push(super::player_gore::Gore {
            entity: target,
            start_time: event.server_time,
            life_ms: 10_000 + (event.receive_sequence.wrapping_mul(7919) % 10_001) as i32,
            shader,
            surfaces,
        });
    }

    /// Draw this entity's burn marks through the same GPU skinning as its body.
    fn append_gore(&mut self, entity: u16, now: i32, draws: &mut Vec<DynamicModelSurface>) {
        self.gore.retain(|gore| now - gore.start_time < gore.life_ms);
        let marks: Vec<_> = self
            .gore
            .iter()
            .filter(|gore| gore.entity == entity && now >= gore.start_time)
            .map(|gore| (gore.shader, gore.life_ms - (now - gore.start_time), gore.surfaces.clone()))
            .collect();
        let mut extra = Vec::new();
        for (shader, remaining, surfaces) in marks {
            let fade = if remaining < 1000 { remaining as f32 / 1000.0 } else { 1.0 };
            let (texture, alpha_mode) = self.custom_shader_material(shader);
            for surface in surfaces {
                let Some(body) = draws.iter().find(|draw| {
                    draw.ghoul2_gpu.as_ref().is_some_and(|skin| *skin.mesh_key == *surface.base_key)
                }) else {
                    continue;
                };
                let Some(skin) = body.ghoul2_gpu.as_ref() else { continue };
                let color = [1.0, 1.0, 1.0, fade];
                extra.push(DynamicModelSurface {
                    entity_num: body.entity_num,
                    wireframe_class: DynamicWireframeClass::Player,
                    raster_visible: body.raster_visible,
                    vertices: Arc::clone(&body.vertices),
                    indices: Arc::clone(&surface.indices),
                    lighting_origin: body.lighting_origin,
                    rt_rigid: None,
                    rt_skinned_key: None,
                    ghoul2_gpu: Some(Ghoul2GpuSkinning {
                        mesh_key: Arc::clone(&surface.key),
                        vertices: Arc::clone(&surface.vertices),
                        indices: Arc::clone(&surface.indices),
                        bones: Arc::clone(&skin.bones),
                        axis: skin.axis,
                        origin: skin.origin,
                        color,
                        uv_xform: [1.0, 1.0, 0.0, 0.0],
                        specular: None,
                        bulge_height: 0.0,
                        env_map: false,
                        jiggle_offsets: skin.jiggle_offsets,
                    }),
                    fx_gpu_sprites: None,
                    texture: texture.clone(),
                    alpha_mode: blend_for_alpha(alpha_mode, fade),
                });
            }
        }
        draws.extend(extra);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn set_jiggle_config(
        &mut self,
        enabled: bool,
        hz: u32,
        max_substeps: u32,
        overall_strength: f32,
        breast_strength: f32,
        glute_strength: f32,
        stiffness_scale: f32,
        damping_scale: f32,
        glute_lift: f32,
    ) {
        self.jiggle.set_config(
            enabled,
            hz,
            max_substeps,
            super::jiggle::JiggleTuning {
                overall_strength,
                breast_strength,
                glute_strength,
                stiffness_scale,
                damping_scale,
                glute_lift,
            },
        );
    }

    pub(crate) fn set_cloth_config(&mut self, config: ClothConfig) {
        self.cloth.set_config(config);
    }

    pub(crate) fn set_cloth_wind(&mut self, wind: Option<crate::ocean::OceanWind>) {
        self.cloth_weather_wind = wind;
    }

    pub fn set_ragdoll_config(&mut self, config: RagdollConfig) {
        let previous_dismemberment = self.ragdolls.dismemberment_level();
        let dismemberment = config.dismemberment;
        self.ragdolls.set_config(config);
        if previous_dismemberment != dismemberment {
            // Surface mutations are presentation state. Rebuild them from the
            // currently transmitted G2_MODEL_PART entities on the next frame.
            self.dismembered.clear();
            self.dismember_source_snaps.clear();
            self.detached_limb_visuals.clear();
        }
    }

    pub fn set_physics_map_mesh(&mut self, mesh: &PhysicsMapMesh) -> Result<(), String> {
        self.ragdolls.set_map_mesh(mesh)
    }

    /// Resolve which living entity is currently being Force-gripped. Vanilla
    /// multiplayer does not send forceGripEntityNum in entityState, so remote
    /// targets are reconstructed from the same information the original server
    /// uses for acquisition: active FP_GRIP, full view angles, and a 256-unit
    /// forward trace. Existing targets are preserved while they stay valid so
    /// level-3 grip can swing the victim without target jitter.
    ///
    /// `local_force_gripped` comes from cg.snap/predicted ps.fd.forceGripCripple
    /// and is exact for the local/followed player.
    pub fn prepare_force_grip_targets(
        &mut self,
        entities: &[PresentedEntity],
        followed: Option<&PresentedEntity>,
        local_force_gripped: bool,
        current_time: i32,
    ) {
        let mut all = entities
            .iter()
            .filter(|entity| entity.entity_type == ET_PLAYER || entity.entity_type == ET_NPC)
            .collect::<Vec<_>>();
        if let Some(followed) = followed {
            if !all.iter().any(|entity| entity.number == followed.number) {
                all.push(followed);
            }
        }

        let by_number = all
            .iter()
            .map(|entity| (entity.number, *entity))
            .collect::<HashMap<_, _>>();
        let active_grippers = all
            .iter()
            .copied()
            .filter(|entity| {
                entity.state.field_i32("eFlags").unwrap_or(0) & EF_DEAD == 0
                    && entity.state.field_i32("forcePowersActive").unwrap_or(0) & (1 << FP_GRIP) != 0
            })
            .collect::<Vec<_>>();

        let mut next_targets = HashMap::new();
        let mut gripped = HashSet::new();

        for gripper in active_grippers {
            let preserved = self
                .force_grip_targets
                .get(&gripper.number)
                .and_then(|target_num| by_number.get(target_num).copied())
                .filter(|target| force_grip_target_still_valid(gripper, target));
            let target = preserved.or_else(|| infer_force_grip_target(gripper, &all));
            if let Some(target) = target {
                next_targets.insert(gripper.number, target.number);
                gripped.insert(target.number);
            }
        }

        if local_force_gripped {
            if let Some(followed) = followed {
                gripped.insert(followed.number);
            }
        }

        let released = self
            .force_gripped_entities
            .difference(&gripped)
            .copied()
            .collect::<Vec<_>>();
        for entity in &released {
            self.ragdolls.begin_living_recovery(*entity, current_time);
        }

        if self.ragdolls.debug_enabled() {
            for entity in gripped.difference(&self.force_gripped_entities) {
                let gripper = next_targets
                    .iter()
                    .find_map(|(gripper, target)| (*target == *entity).then_some(*gripper));
                println!(
                    "RAPIER FORCE GRIP START: victim={} gripper={}",
                    entity,
                    gripper.map_or_else(|| "local-playerstate".to_owned(), |value| value.to_string()),
                );
            }
            for entity in &released {
                println!("RAPIER FORCE GRIP END: victim={entity}");
            }
        }
        self.force_grip_targets = next_targets;
        self.force_gripped_entities = gripped;
    }

    /// Resolve presentation-only impulse ragdolls from authoritative gameplay
    /// evidence. The local/followed player can use PMF_TIME_KNOCKBACK exactly;
    /// remote entityState cannot, so remote activation requires both a known
    /// impact/Force candidate and a matching server trajectory-velocity change.
    pub fn prepare_impulse_ragdolls(
        &mut self,
        entities: &[PresentedEntity],
        followed: Option<&PresentedEntity>,
        local_knockback: bool,
        current_time: i32,
    ) {
        let mut all = entities
            .iter()
            .filter(|entity| entity.entity_type == ET_PLAYER || entity.entity_type == ET_NPC)
            .collect::<Vec<_>>();
        if let Some(followed) = followed {
            if !all.iter().any(|entity| entity.number == followed.number) {
                all.push(followed);
            }
        }
        let live = all.iter().map(|entity| entity.number).collect::<HashSet<_>>();
        let followed_num = followed.map(|entity| entity.number);

        if !self.ragdolls.active() {
            self.pending_impulse_ragdolls.clear();
            self.active_impulse_ragdolls.clear();
            self.explosion_impulse_pulses.clear();
            self.previous_impulse_velocity.clear();
            self.previous_impulse_velocity.extend(
                all.iter().map(|entity| (entity.number, presented_entity_velocity(entity))),
            );
            self.force_gesture_anim.clear();
            self.force_gesture_anim.extend(
                all.iter().map(|entity| {
                    (entity.number, entity.state.field_i32("torsoAnim").unwrap_or(0))
                }),
            );
            return;
        }

        self.pending_impulse_ragdolls
            .retain(|entity, candidate| live.contains(entity) && current_time <= candidate.expires_at);
        self.explosion_impulse_pulses
            .retain(|pulse| current_time <= pulse.expires_at);
        self.previous_impulse_velocity.retain(|entity, _| live.contains(entity));
        self.force_gesture_anim.retain(|entity, _| live.contains(entity));

        // Explosion events do not enumerate splash victims. Treat nearby living
        // players as candidates only; a trajectory discontinuity is still
        // required below before a ragdoll is activated.
        if self.ragdolls.explosion_impulses_enabled() {
            let pulses = self.explosion_impulse_pulses.clone();
            let mut candidates = Vec::new();
            for pulse in pulses {
                for target in &all {
                    if !force_grip_target_alive(target) {
                        continue;
                    }
                    let delta = Vec3::from_array(target.origin) - Vec3::from_array(pulse.origin);
                    if delta.length_squared() <= EXPLOSION_CANDIDATE_RADIUS.powi(2) {
                        candidates.push((target.number, pulse.origin));
                    }
                }
            }
            for (target, origin) in candidates {
                self.queue_impulse_candidate(
                    target,
                    ImpulseRagdollSource::Explosion,
                    origin,
                    false,
                    current_time,
                );
            }
        }

        // ForceThrow itself has no victim event in protocol 26. OpenJK does,
        // however, drive BOTH_FORCEPUSH/BOTH_FORCEPULL through the networked
        // torsoAnim. Use the animation *transition* to open a short candidate
        // window, then require the victim's authoritative velocity to change in
        // the matching push/pull direction.
        if self.ragdolls.force_impulses_enabled() {
            let mut gestures = Vec::new();
            for source in &all {
                let raw = source.state.field_i32("torsoAnim").unwrap_or(0);
                let animation = AnimationSet::name(raw & !ANIM_TOGGLEBIT);
                let previous = self.force_gesture_anim.insert(source.number, raw);
                if previous == Some(raw) {
                    continue;
                }
                let toward_source = match animation {
                    Some("BOTH_FORCEPUSH") => false,
                    Some("BOTH_FORCEPULL") => true,
                    _ => continue,
                };
                gestures.push((source.number, source.origin, source.angles, toward_source));
            }

            let mut candidates = Vec::new();
            for (source_num, source_origin, source_angles, toward_source) in gestures {
                let forward = Vec3::from_array(angles_to_axis(source_angles)[0]).normalize_or_zero();
                for target in &all {
                    if target.number == source_num || !force_grip_target_alive(target) {
                        continue;
                    }
                    let offset = Vec3::from_array(target.origin) - Vec3::from_array(source_origin);
                    let distance = offset.length();
                    if distance <= 1.0 || distance > FORCE_CANDIDATE_RADIUS {
                        continue;
                    }
                    // ForceThrow's broad target pass ultimately requires a
                    // forward dot >= 0.6 for clients. Level-1's exact trace is
                    // narrower, so this remains a candidate rather than proof.
                    if forward.dot(offset / distance) < 0.6 {
                        continue;
                    }
                    candidates.push((target.number, source_origin, toward_source));
                }
            }
            for (target, origin, toward_origin) in candidates {
                self.queue_impulse_candidate(
                    target,
                    ImpulseRagdollSource::Force,
                    origin,
                    toward_origin,
                    current_time,
                );
            }
        } else {
            // Still keep transition history current so re-enabling the option
            // doesn't synthesize an old Force gesture.
            for source in &all {
                self.force_gesture_anim.insert(
                    source.number,
                    source.state.field_i32("torsoAnim").unwrap_or(0),
                );
            }
        }

        let mut activations = Vec::new();
        for target in &all {
            let velocity = presented_entity_velocity(target);
            let previous = self.previous_impulse_velocity.get(&target.number).copied();
            let candidate = self.pending_impulse_ragdolls.get(&target.number).copied();
            let exact_local_knockback = followed_num == Some(target.number) && local_knockback;

            if !self.force_gripped_entities.contains(&target.number)
                && force_grip_target_alive(target)
            {
                if let (Some(candidate), Some(previous)) = (candidate, previous) {
                    let velocity_delta = Vec3::from_array(velocity) - Vec3::from_array(previous);
                    let delta_len = velocity_delta.length();
                    let target_from_origin = Vec3::from_array(target.origin) - Vec3::from_array(candidate.origin);
                    let expected = if candidate.toward_origin {
                        -target_from_origin
                    } else {
                        target_from_origin
                    }
                    .normalize_or_zero();
                    let direction_matches = expected.length_squared() < 1.0e-6
                        || delta_len < 1.0e-6
                        || velocity_delta.normalize_or_zero().dot(expected) >= 0.20;
                    if exact_local_knockback
                        || (delta_len >= IMPULSE_DELTA_MIN && direction_matches)
                    {
                        activations.push((target.number, candidate.source, previous));
                    }
                }
            }
            self.previous_impulse_velocity.insert(target.number, velocity);
        }

        for (entity, source, seed_velocity) in activations {
            self.pending_impulse_ragdolls.remove(&entity);
            let expires_at = current_time.saturating_add(IMPULSE_RAGDOLL_MS);
            let was_active = self.active_impulse_ragdolls.contains_key(&entity);
            self.active_impulse_ragdolls
                .entry(entity)
                .and_modify(|active| {
                    active.expires_at = active.expires_at.max(expires_at);
                    active.source = source;
                })
                .or_insert(ActiveImpulseRagdoll {
                    source,
                    expires_at,
                    seed_velocity,
                });
            if self.ragdolls.debug_enabled() && !was_active {
                println!(
                    "RAPIER IMPULSE START: victim={} source={:?} seedVelocity=[{:.1},{:.1},{:.1}]",
                    entity, source, seed_velocity[0], seed_velocity[1], seed_velocity[2]
                );
            }
        }

        // OpenJK's BG_InKnockDownOnly is BOTH_KNOCKDOWN1..5. Once the
        // authoritative entity enters one of those animations, stop treating
        // the living player as a free impulse reaction and hand the exact
        // rendered Rapier pose into that already-advancing Ghoul2 animation.
        //
        // Require an existing Rapier instance so an impact and knockdown that
        // arrive in the same snapshot still get at least one genuine physics
        // presentation frame before the blend begins.
        let knockdown_handoffs = all
            .iter()
            .filter_map(|entity| {
                (self.active_impulse_ragdolls.contains_key(&entity.number)
                    && presented_entity_in_knockdown(entity)
                    && self.ragdolls.has_instance(entity.number))
                    .then_some(entity.number)
            })
            .collect::<Vec<_>>();
        for entity in knockdown_handoffs {
            self.active_impulse_ragdolls.remove(&entity);
            self.pending_impulse_ragdolls.remove(&entity);
            self.ragdolls.begin_knockdown_recovery(entity, current_time);
            if self.ragdolls.debug_enabled() {
                println!("RAPIER IMPULSE -> KNOCKDOWN: victim={entity}");
            }
        }

        let expired = self
            .active_impulse_ragdolls
            .iter()
            .filter_map(|(&entity, active)| {
                (current_time >= active.expires_at || !live.contains(&entity)).then_some(entity)
            })
            .collect::<Vec<_>>();
        for entity in expired {
            self.active_impulse_ragdolls.remove(&entity);
            if live.contains(&entity) && !self.force_gripped_entities.contains(&entity) {
                self.ragdolls.begin_living_recovery(entity, current_time);
            }
            if self.ragdolls.debug_enabled() {
                println!("RAPIER IMPULSE END: victim={entity}");
            }
        }

        // Grip has stricter presentation semantics and always wins immediately.
        for entity in &self.force_gripped_entities {
            self.active_impulse_ragdolls.remove(entity);
            self.pending_impulse_ragdolls.remove(entity);
        }
    }

    /// Advance the visual physics world exactly once for this presentation frame.
    /// Ragdolls spawned later in the frame start from the exact current Ghoul2 pose.
    pub fn begin_physics_frame(&mut self, current_time: i32) {
        self.ragdolls.begin_frame(current_time);
    }

    pub fn perf_stats(&self) -> Ghoul2PerfStats {
        self.perf
    }

    pub fn set_skinning_mode(&mut self, mode: Ghoul2SkinningMode) {
        self.skinning_mode = mode;
    }

    pub fn skinning_mode(&self) -> Ghoul2SkinningMode {
        self.skinning_mode
    }

    pub fn set_early_frustum_cull(&mut self, enabled: bool) {
        self.early_frustum_cull = enabled;
    }

    /// Keep auxiliary presentation-only player runtimes alive when the ordinary
    /// snapshot player pass prunes stale centity state.
    pub fn set_external_live_entities(
        &mut self,
        entities: impl IntoIterator<Item = u16>,
    ) {
        self.external_live_entities.clear();
        self.external_live_entities.extend(entities);
    }

    pub fn set_rt_shadow_casters_enabled(&mut self, enabled: bool) {
        self.rt_shadow_casters_enabled = enabled;
    }

    /// Install the current map collision world for OpenJK-style blob traces.
    /// This is map-scoped and must not be rebound every frame.
    pub fn set_collision_world(&mut self, world: Option<CollisionWorld>) {
        self.collision_world = world;
        self.blob_shadow_instances.clear();
    }

    /// Begin one CG_AddEntities blob-shadow collection pass. All eligible
    /// players/NPCs are packed into one dynamic surface so the OpenJK visual
    /// costs one draw rather than one temporary-poly draw per character.
    pub fn begin_blob_shadow_frame(&mut self, enabled: bool) {
        self.blob_shadows_enabled = enabled;
        self.blob_shadow_instances.clear();
    }

    /// Finish cg_shadows 1. Keep the stock `markShadow` image *and its authored
    /// first-stage blend mode*, while batching all blobs into one surface.
    pub fn finish_blob_shadow_frame(&mut self) -> Option<DynamicModelSurface> {
        if !self.blob_shadows_enabled || self.blob_shadow_instances.is_empty() {
            return None;
        }
        if !self.blob_shadow_texture_resolved {
            let (texture, alpha_mode) = self.custom_shader_material("markShadow");
            self.blob_shadow_texture = texture;
            self.blob_shadow_alpha_mode = alpha_mode;
            self.blob_shadow_texture_resolved = true;
        }
        let Some(texture) = self.blob_shadow_texture.as_ref().map(Arc::clone) else {
            if !self.blob_shadow_asset_warned {
                eprintln!("BLOB SHADOW: OpenJK markShadow material/image was not found; suppressing blob draw");
                self.blob_shadow_asset_warned = true;
            }
            self.blob_shadow_instances.clear();
            return None;
        };
        Some(DynamicModelSurface {
            entity_num: u16::MAX,
            wireframe_class: DynamicWireframeClass::Effect,
            raster_visible: true,
            vertices: Arc::new(Vec::new()),
            indices: Arc::new(Vec::new()),
            lighting_origin: None,
            rt_rigid: None,
            rt_skinned_key: None,
            ghoul2_gpu: None,
            fx_gpu_sprites: Some(FxGpuSprites {
                instances: Arc::new(std::mem::take(&mut self.blob_shadow_instances)),
                blob_shadow: true,
            }),
            texture: Some(texture),
            // OpenJK registers `markShadow` as a shader and lets the renderer
            // honor the shader's authored blendFunc; preserve that here.
            alpha_mode: self.blob_shadow_alpha_mode,
        })
    }

    pub fn set_lod_scale(&mut self, lod_scale: f32) {
        self.lod_scale = if lod_scale.is_finite() {
            lod_scale.clamp(crate::fx::LOD_SCALE_MIN, crate::fx::LOD_SCALE_MAX)
        } else {
            crate::fx::LOD_SCALE_DEFAULT
        };
    }

    pub fn set_lod_bias(&mut self, lod_bias: i32) {
        // OpenJK's renderer cvar is not range-checked, but its Ghoul2 path
        // takes max(r_lodbias, modelBias). Our current per-model bias is 0,
        // so negative values have no effect; keep the stored runtime value in
        // the useful non-negative range.
        self.lod_bias = lod_bias.max(0);
    }

    /// `r_ghoul2animsmooth`. jaPRO stores this cvar unclamped and only enables
    /// smoothing when it reads strictly inside `(0, 1)` at use
    /// (`G2_TransformGhoulBones`'s `val>0.0f&&val<1.0f` gate); `1.0` or higher
    /// is therefore a no-op in jaPRO, not "maximum smoothing". Mirror that by
    /// storing the raw value here and gating activation at the call site,
    /// rather than clamping into `[0, 1)` and making `1.0` mean "near-frozen".
    pub fn set_ghoul2_anim_smooth(&mut self, factor: f32) {
        self.ghoul2_anim_smooth = if factor.is_finite() {
            factor.max(0.0)
        } else {
            0.0
        };
    }

    fn cloth_surface_name<'a>(glm: &'a GlmModel, surface: &GlmSurface) -> Option<&'a str> {
        glm.hierarchy
            .get(surface.surface_index)
            .map(|entry| entry.name.as_str())
    }

    fn cloth_body_capsules(
        &mut self, model_label: &str, glm: &GlmModel,
        surface_assets: &[PlayerSurfaceAsset], lod_index: usize,
        gla: &GlaAnimation, pose: &[Matrix3x4],
    ) -> Vec<ClothCapsule> {
        let visible = surface_assets.iter().map(|asset| asset.surface_index).collect::<Vec<_>>();
        let key = (model_label.to_owned(), lod_index, visible);
        let templates = self.cloth_body_templates.entry(key.clone()).or_insert_with(||
            super::cloth_body::build_body_templates(glm, gla, &key.2, lod_index));
        super::cloth_body::pose_body_capsules(templates, gla, pose)
    }
    fn cloth_surface_frame(
        glm: &GlmModel,
        surface: &GlmSurface,
        skinned: &Ghoul2SkinnedSurface,
        pose: &[Matrix3x4],
    ) -> Option<ClothSurfaceFrame> {
        let surface_name = Self::cloth_surface_name(glm, surface)?;
        if !ClothSystem::is_cloth_surface_name(surface_name) {
            return None;
        }
        // These transforms let the garment solver derive attachment motion
        // from the actual skin weights. No bone names or animation IDs enter
        // the rule for how hanging fabric follows those attachments.
        let skin_transforms = surface.vertices.iter().map(|vertex| {
            let mut matrix = [[0.0; 4]; 3];
            for weight in &vertex.weights {
                let index = *surface.bone_references.get(weight.local_bone_index)?;
                let bone = pose.get(index)?;
                for row in 0..3 {
                    for column in 0..4 {
                        matrix[row][column] += bone[row][column] * weight.weight;
                    }
                }
            }
            Some(matrix)
        }).collect::<Option<Vec<_>>>()?;
        Some(ClothSurfaceFrame {
            surface_index: surface.surface_index,
            surface_name: surface_name.to_owned(),
            bind_positions: surface.vertices.iter().map(|vertex| vertex.position).collect(),
            posed_positions: skinned.vertices.iter().map(|vertex| vertex.position).collect(),
            posed_normals: skinned.vertices.iter().map(|vertex| vertex.normal).collect(),
            skin_transforms,
            triangles: surface.triangles.clone(),
        })
    }


    fn simulate_cloth_frames(
        &mut self,
        entity_num: u16,
        model_label: &str,
        lod_index: usize,
        frames: &[ClothSurfaceFrame],
        glm: &GlmModel,
        surface_assets: &[PlayerSurfaceAsset],
        gla: &GlaAnimation,
        pose: &[Matrix3x4],
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
    ) -> HashMap<usize, ClothOutput> {
        if !self.cloth.enabled() || frames.is_empty() {
            return HashMap::new();
        }
        let capsules = self.cloth_body_capsules(model_label, glm, surface_assets, lod_index, gla, pose);
        let wind = self.cloth_weather_wind.map(|wind| {
            let velocity = wind.at(self.stage_time_ms as f32 * 0.001);
            // Shared weather uses render X/Z; cloth uses JKA X/Y/Z.
            scene::jka_position([velocity[0], 0.0, velocity[1]])
        }).unwrap_or([0.0; 3]);
        self.cloth.set_wind_velocity(wind);
        self.cloth
            .simulate_garment(
                entity_num,
                model_label,
                lod_index,
                frames,
                &capsules,
                ClothMotion { axis, origin },
                self.stage_time_ms,
            )
            .unwrap_or_default()
    }

    fn cpu_surface_vertices_with_cloth(
        skinned: Ghoul2SkinnedSurface,
        cloth_output: Option<ClothOutput>,
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        color: [f32; 4],
    ) -> Vec<DynamicModelVertex> {
        if let Some(output) = cloth_output {
            if output.positions.len() == skinned.vertices.len()
                && output.normals.len() == skinned.vertices.len()
            {
                return skinned
                    .vertices
                    .into_iter()
                    .zip(output.positions)
                    .zip(output.normals)
                    .map(|((vertex, position), normal)| DynamicModelVertex {
                        position: transform_model_point(position, axis, origin),
                        normal: transform_model_normal(normal, axis),
                        uv: vertex.uv,
                        color,
                        depth_hack: 0.0,
                    })
                    .collect();
            }
        }

        skinned
            .vertices
            .into_iter()
            .map(|vertex| DynamicModelVertex {
                position: transform_model_point(vertex.position, axis, origin),
                normal: transform_model_normal(vertex.normal, axis),
                uv: vertex.uv,
                color,
                depth_hack: 0.0,
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_glm_surfaces(
        &mut self,
        entity_num: u16,
        model_label: &str,
        glm: &GlmModel,
        gla: &GlaAnimation,
        surface_assets: &[PlayerSurfaceAsset],
        jiggle_profile: Option<&JiggleProfile>,
        pose: &[Matrix3x4],
        lod_index: usize,
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        rgba: [f32; 4],
        custom_material: Option<(Option<Arc<TextureData>>, DynamicModelAlphaMode)>,
        apply_alpha_blend: bool,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        self.render_glm_surfaces_tinted(
            entity_num,
            model_label,
            glm,
            gla,
            surface_assets,
            jiggle_profile,
            pose,
            lod_index,
            axis,
            origin,
            rgba,
            [1.0; 3],
            custom_material,
            apply_alpha_blend,
        )
    }

    /// `render_glm_surfaces` plus the refEntity `shaderRGBA` tint of a player
    /// (`customRGBA`, i.e. `char_color_*`). Only shaders that read the entity
    /// colour (`rgbGen lightingDiffuseEntity`) follow it; their untinted overlay
    /// stage is drawn over them, so just the texture's alpha-masked regions change.
    #[allow(clippy::too_many_arguments)]
    fn render_glm_surfaces_tinted(
        &mut self,
        entity_num: u16,
        model_label: &str,
        glm: &GlmModel,
        gla: &GlaAnimation,
        surface_assets: &[PlayerSurfaceAsset],
        jiggle_profile: Option<&JiggleProfile>,
        pose: &[Matrix3x4],
        lod_index: usize,
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        rgba: [f32; 4],
        entity_rgb: [f32; 3],
        custom_material: Option<(Option<Arc<TextureData>>, DynamicModelAlphaMode)>,
        apply_alpha_blend: bool,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        self.render_glm_surfaces_tinted_selected(
            entity_num, model_label, glm, gla, surface_assets, jiggle_profile, pose, lod_index,
            axis, origin, rgba, entity_rgb, custom_material, apply_alpha_blend, None,
        )
    }

    fn render_glm_surfaces_tinted_selected(
        &mut self,
        entity_num: u16,
        model_label: &str,
        glm: &GlmModel,
        gla: &GlaAnimation,
        surface_assets: &[PlayerSurfaceAsset],
        jiggle_profile: Option<&JiggleProfile>,
        pose: &[Matrix3x4],
        lod_index: usize,
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        rgba: [f32; 4],
        entity_rgb: [f32; 3],
        custom_material: Option<(Option<Arc<TextureData>>, DynamicModelAlphaMode)>,
        apply_alpha_blend: bool,
        surface_selection: Option<&HashSet<usize>>,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let tinted = entity_rgb != [1.0; 3] && custom_material.is_none();
        let stage_seconds = self.stage_time_ms as f32 * 0.001;
        let stage_frame = StageFrame { axis, origin, viewer: self.stage_view_position };
        let lod_index = lod_index.min(glm.lods.len().saturating_sub(1));
        let lod = glm
            .lods
            .get(lod_index)
            .ok_or_else(|| format!("{model_label} has no GLM LODs"))?;
        let jobs = surface_assets
            .iter()
            .filter_map(|asset| {
                let visible = surface_selection
                    .map_or(asset.default_visible, |selection| selection.contains(&asset.surface_index));
                if !visible {
                    return None;
                }
                let surface = lod
                    .surfaces
                    .iter()
                    .find(|surface| surface.surface_index == asset.surface_index)?;
                let gpu_mesh = asset.gpu_meshes.get(lod_index)?.as_ref()?;
                Some((asset, surface, gpu_mesh))
            })
            .collect::<Vec<_>>();

        let jiggle_profile = jiggle_profile.filter(|_| self.jiggle.enabled());
        let jiggle_offsets = jiggle_profile.map(|profile| {
            self.jiggle.simulate(
                entity_num,
                model_label,
                profile,
                pose,
                axis,
                origin,
                self.stage_time_ms,
            )
        });
        let gpu_jiggle_offsets = if let (Some(profile), Some(offsets)) =
            (jiggle_profile, jiggle_offsets.as_deref())
        {
            if profile.gpu_supported() {
                profile.gpu_offsets(offsets, self.jiggle.tuning())
            } else {
                [[0.0; 4]; 4]
            }
        } else {
            [[0.0; 4]; 4]
        };

        if self.skinning_mode == Ghoul2SkinningMode::Gpu {
            let bones = gpu_bones_from_pose(pose);
            let empty_vertices = Arc::new(Vec::new());

            // Cloth remains a CPU post-skin deformation path. Jiggle stays on
            // GPU whenever its profile fits the four-region vertex payload; only
            // cloth (or an oversized explicit jiggle profile) falls back to CPU.
            let mut deformed_skinned = HashMap::<usize, Ghoul2SkinnedSurface>::new();
            let mut cloth_frames = Vec::<ClothSurfaceFrame>::new();
            for (_, surface, _) in &jobs {
                let cloth_candidate = self.cloth.enabled()
                    && Self::cloth_surface_name(glm, surface)
                        .is_some_and(ClothSystem::is_cloth_surface_name);
                let jiggle_candidate = jiggle_profile
                    .is_some_and(|profile| profile.affects_surface(lod_index, surface.surface_index));
                let jiggle_cpu_fallback = jiggle_candidate
                    && jiggle_profile.is_some_and(|profile| !profile.gpu_supported());
                if !cloth_candidate && !jiggle_cpu_fallback {
                    continue;
                }

                let mut skinned = skin_surface_timed(&mut self.perf, surface, pose)?;
                if jiggle_candidate {
                    if let (Some(profile), Some(offsets)) = (jiggle_profile, jiggle_offsets.as_deref()) {
                        profile.deform_surface(
                            lod_index,
                            surface.surface_index,
                            offsets,
                            self.jiggle.tuning(),
                            &mut skinned,
                        );
                    }
                }
                if cloth_candidate {
                    if let Some(frame) = Self::cloth_surface_frame(glm, surface, &skinned, pose) {
                        cloth_frames.push(frame);
                    }
                }
                deformed_skinned.insert(surface.surface_index, skinned);
            }
            let mut cloth_outputs =
                self.simulate_cloth_frames(entity_num, model_label, lod_index, &cloth_frames, glm, surface_assets, gla, pose, axis, origin);

            let mut draws = Vec::with_capacity(jobs.len());
            for (asset, surface, gpu_mesh) in jobs {
                let gpu_jiggle_surface = jiggle_profile.is_some_and(|profile| {
                    profile.gpu_supported()
                        && profile.affects_surface(lod_index, surface.surface_index)
                });
                let (draw_key, draw_vertices, draw_indices) = if gpu_jiggle_surface {
                    (&gpu_mesh.key, &gpu_mesh.vertices, &gpu_mesh.indices)
                } else {
                    (&gpu_mesh.base_key, &gpu_mesh.base_vertices, &gpu_mesh.cpu_indices)
                };
                if draw_vertices.is_empty() || draw_indices.is_empty() {
                    continue;
                }
                let (texture, base_alpha_mode) = custom_material
                    .clone()
                    .unwrap_or_else(|| (asset.texture.clone(), asset.alpha_mode));
                let surface_rgba = if custom_material.is_none() && asset.fallback_gray {
                    [0.58, 0.58, 0.58, rgba[3]]
                } else if tinted && asset.entity_tint {
                    [entity_rgb[0], entity_rgb[1], entity_rgb[2], rgba[3]]
                } else {
                    rgba
                };
                let alpha_mode = if apply_alpha_blend {
                    blend_for_alpha(base_alpha_mode, surface_rgba[3])
                } else {
                    base_alpha_mode
                };
                let overlay_at = draws.len() + 1;

                if let Some(skinned) = deformed_skinned.remove(&surface.surface_index) {
                    let vertices = Self::cpu_surface_vertices_with_cloth(
                        skinned,
                        cloth_outputs.remove(&surface.surface_index),
                        axis,
                        origin,
                        surface_rgba,
                    );
                    if !vertices.is_empty() {
                        draws.push(DynamicModelSurface {
                            entity_num,
                            wireframe_class: DynamicWireframeClass::Player,
                            raster_visible: true,
                            vertices: Arc::new(vertices),
                            indices: Arc::clone(&gpu_mesh.cpu_indices),
                            lighting_origin: Some(origin),
                            rt_rigid: None,
                            rt_skinned_key: self.rt_shadow_casters_enabled.then(|| Arc::clone(&gpu_mesh.base_key)),
                            ghoul2_gpu: None,
                            fx_gpu_sprites: None,
                            texture,
                            alpha_mode,
                        });
                        if tinted && asset.entity_tint {
                            Self::push_tint_overlay(&mut draws, overlay_at, asset, rgba[3]);
                        }
                        if custom_material.is_none() {
                            Self::push_stage_draws(&mut draws, overlay_at, asset, rgba[3], stage_seconds, &stage_frame);
                        }
                    }
                    continue;
                }

                self.perf.surfaces_skinned = self.perf.surfaces_skinned.saturating_add(1);
                self.perf.vertices_skinned = self
                    .perf
                    .vertices_skinned
                    .saturating_add(draw_vertices.len() as u64);
                draws.push(DynamicModelSurface {
                    entity_num,
                    wireframe_class: DynamicWireframeClass::Player,
                    raster_visible: true,
                    vertices: Arc::clone(&empty_vertices),
                    indices: Arc::clone(draw_indices),
                    lighting_origin: Some(origin),
                    rt_rigid: None,
                    rt_skinned_key: self.rt_shadow_casters_enabled.then(|| Arc::clone(draw_key)),
                    ghoul2_gpu: Some(Ghoul2GpuSkinning {
                        mesh_key: Arc::clone(draw_key),
                        vertices: Arc::clone(draw_vertices),
                        indices: Arc::clone(draw_indices),
                        bones: Arc::clone(&bones),
                        axis,
                        origin,
                        color: surface_rgba,
                        uv_xform: [1.0, 1.0, 0.0, 0.0],
                        specular: None,
                        bulge_height: 0.0,
                        env_map: false,
                        jiggle_offsets: gpu_jiggle_offsets,
                    }),
                    fx_gpu_sprites: None,
                    texture,
                    alpha_mode,
                });
                if tinted && asset.entity_tint {
                    Self::push_tint_overlay(&mut draws, overlay_at, asset, rgba[3]);
                }
                if custom_material.is_none() {
                    Self::push_stage_draws(&mut draws, overlay_at, asset, rgba[3], stage_seconds, &stage_frame);
                }
            }
            return Ok(draws);
        }

        let used_workers = self.skinning_mode == Ghoul2SkinningMode::CpuWorkers && jobs.len() > 1;
        let mut skinned = if used_workers {
            let started = Instant::now();
            let results = self.skinning_pool.install(|| {
                jobs.par_iter()
                    .map(|(_, surface, _)| skin_glm_surface(surface, pose))
                    .collect::<Vec<_>>()
            });
            self.perf.skin_ms += started.elapsed().as_secs_f64() * 1000.0;
            results
        } else {
            jobs.iter()
                .map(|(_, surface, _)| skin_surface_timed(&mut self.perf, surface, pose))
                .collect::<Vec<_>>()
        };

        if let (Some(profile), Some(offsets)) = (jiggle_profile, jiggle_offsets.as_deref()) {
            for ((_, surface, _), result) in jobs.iter().zip(skinned.iter_mut()) {
                if !profile.affects_surface(lod_index, surface.surface_index) {
                    continue;
                }
                if let Ok(skinned_surface) = result {
                    profile.deform_surface(
                        lod_index,
                        surface.surface_index,
                        offsets,
                        self.jiggle.tuning(),
                        skinned_surface,
                    );
                }
            }
        }

        let cloth_frames = if self.cloth.enabled() {
            jobs.iter()
                .zip(skinned.iter())
                .filter_map(|((_, surface, _), skinned)| {
                    let skinned = skinned.as_ref().ok()?;
                    Self::cloth_surface_frame(glm, surface, skinned, pose)
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let mut cloth_outputs =
            self.simulate_cloth_frames(entity_num, model_label, lod_index, &cloth_frames, glm, surface_assets, gla, pose, axis, origin);

        let mut draws = Vec::with_capacity(jobs.len());
        for ((asset, surface, gpu_mesh), skinned) in jobs.into_iter().zip(skinned) {
            let skinned = skinned?;
            let surface_rgba = if custom_material.is_none() && asset.fallback_gray {
                [0.58, 0.58, 0.58, rgba[3]]
            } else if tinted && asset.entity_tint {
                [entity_rgb[0], entity_rgb[1], entity_rgb[2], rgba[3]]
            } else {
                rgba
            };
            if used_workers {
                self.perf.surfaces_skinned = self.perf.surfaces_skinned.saturating_add(1);
                self.perf.vertices_skinned = self
                    .perf
                    .vertices_skinned
                    .saturating_add(skinned.vertices.len() as u64);
            }
            let vertices = Self::cpu_surface_vertices_with_cloth(
                skinned,
                cloth_outputs.remove(&surface.surface_index),
                axis,
                origin,
                surface_rgba,
            );
            if vertices.is_empty() || gpu_mesh.cpu_indices.is_empty() {
                continue;
            }
            let (texture, base_alpha_mode) = custom_material
                .clone()
                .unwrap_or_else(|| (asset.texture.clone(), asset.alpha_mode));
            let alpha_mode = if apply_alpha_blend {
                blend_for_alpha(base_alpha_mode, surface_rgba[3])
            } else {
                base_alpha_mode
            };
            let overlay_at = draws.len() + 1;
            draws.push(DynamicModelSurface {
                entity_num,
                wireframe_class: DynamicWireframeClass::Player,
                raster_visible: true,
                vertices: Arc::new(vertices),
                indices: Arc::clone(&gpu_mesh.cpu_indices),
                lighting_origin: Some(origin),
                rt_rigid: None,
                rt_skinned_key: self.rt_shadow_casters_enabled.then(|| Arc::clone(&gpu_mesh.base_key)),
                ghoul2_gpu: None,
                fx_gpu_sprites: None,
                texture,
                alpha_mode,
            });
            if tinted && asset.entity_tint {
                Self::push_tint_overlay(&mut draws, overlay_at, asset, rgba[3]);
            }
            if custom_material.is_none() {
                Self::push_stage_draws(&mut draws, overlay_at, asset, rgba[3], stage_seconds, &stage_frame);
            }
        }
        Ok(draws)
    }

    /// Turn the surface draw just pushed (`draws[base_at - 1]`) into the first
    /// stage of its blended shader and add one draw per further stage. Shaders
    /// without a stage list (everything opaque/masked) leave the draw alone.
    /// The extra draws repeat geometry that is already a shadow caster.
    fn push_stage_draws(
        draws: &mut Vec<DynamicModelSurface>,
        base_at: usize,
        asset: &PlayerSurfaceAsset,
        alpha: f32,
        seconds: f32,
        frame: &StageFrame,
    ) {
        let Some(first) = asset.stages.first() else { return };
        let Some(base) = draws.get(base_at - 1) else { return };
        let extras = asset
            .stages
            .iter()
            .skip(1)
            .map(|stage| {
                let mut surface = Self::clone_surface_with_material(
                    base,
                    stage.texture.clone(),
                    stage.alpha_mode,
                    [1.0, 1.0, 1.0, alpha],
                );
                surface.rt_rigid = None;
                surface.rt_skinned_key = None;
                Self::apply_stage(&mut surface, stage, alpha, seconds, frame);
                surface
            })
            .collect::<Vec<_>>();
        Self::apply_stage(&mut draws[base_at - 1], first, alpha, seconds, frame);
        draws.extend(extras);
    }

    /// Colour, texture-coordinate transform and lighting of one shader stage.
    /// CPU vertices are rewritten from the surface's own (untransformed) UVs.
    fn apply_stage(
        surface: &mut DynamicModelSurface,
        stage: &ResolvedStage,
        alpha: f32,
        seconds: f32,
        frame: &StageFrame,
    ) {
        let mut color = [stage.rgb[0], stage.rgb[1], stage.rgb[2], alpha * stage.alpha];
        let xform = stage_uv_xform(&stage.tc_mods, seconds);
        let specular = frame.specular_points().filter(|_| stage.specular_alpha);
        if stage.specular_alpha && specular.is_none() {
            // No viewer (asset preview): a constant mid glint instead of none.
            color[3] *= 0.5;
        }
        surface.texture = stage.texture.clone();
        surface.alpha_mode = stage.alpha_mode;
        if let Some(skin) = surface.ghoul2_gpu.as_mut() {
            skin.color = color;
            skin.uv_xform = xform;
            skin.specular = specular;
        } else {
            surface.vertices = Arc::new(
                surface
                    .vertices
                    .iter()
                    .map(|vertex| {
                        let mut color = color;
                        if let Some((light, viewer)) = specular {
                            color[3] *= specular_alpha(vertex.position, vertex.normal, light, viewer);
                        }
                        DynamicModelVertex {
                            uv: [vertex.uv[0] * xform[0] + xform[2], vertex.uv[1] * xform[1] + xform[3]],
                            color,
                            ..*vertex
                        }
                    })
                    .collect(),
            );
        }
        if stage.unlit {
            surface.lighting_origin = None;
        }
    }

    /// Redraw the tinted surface just pushed (`draws[overlay_at - 1]`) with the
    /// shader's untinted alpha-blended stage. It only repeats geometry that is
    /// already a shadow caster, so it carries no shadow keys of its own.
    fn push_tint_overlay(
        draws: &mut Vec<DynamicModelSurface>,
        overlay_at: usize,
        asset: &PlayerSurfaceAsset,
        alpha: f32,
    ) {
        let Some(overlay) = &asset.overlay else { return };
        let Some(base) = draws.get(overlay_at - 1) else { return };
        let mut surface = Self::clone_surface_with_material(
            base,
            overlay.texture.clone(),
            overlay.alpha_mode,
            [1.0, 1.0, 1.0, alpha],
        );
        surface.rt_rigid = None;
        surface.rt_skinned_key = None;
        draws.push(surface);
    }

    fn clone_surface_with_material(
        surface: &DynamicModelSurface,
        texture: Option<Arc<TextureData>>,
        alpha_mode: DynamicModelAlphaMode,
        color: [f32; 4],
    ) -> DynamicModelSurface {
        Self::clone_surface_with_material_xform(surface, texture, alpha_mode, color, None, None, None)
    }

    /// `clone_surface_with_material`, optionally overriding the UV transform,
    /// bulge offset and environment mapping instead of inheriting the base
    /// surface's (a customShader's own `tcMod`/`deformVertexes bulge`/`tcGen
    /// environment` reads the base mesh's natural UVs and normals, same as the
    /// engine).
    #[allow(clippy::too_many_arguments)]
    fn clone_surface_with_material_xform(
        surface: &DynamicModelSurface,
        texture: Option<Arc<TextureData>>,
        alpha_mode: DynamicModelAlphaMode,
        color: [f32; 4],
        uv_xform: Option<[f32; 4]>,
        bulge_height: Option<f32>,
        env_map: Option<bool>,
    ) -> DynamicModelSurface {
        let ghoul2_gpu = surface.ghoul2_gpu.as_ref().map(|skin| Ghoul2GpuSkinning {
            mesh_key: Arc::clone(&skin.mesh_key),
            vertices: Arc::clone(&skin.vertices),
            indices: Arc::clone(&skin.indices),
            bones: Arc::clone(&skin.bones),
            axis: skin.axis,
            origin: skin.origin,
            color,
            uv_xform: uv_xform.unwrap_or(skin.uv_xform),
            specular: skin.specular,
            bulge_height: bulge_height.unwrap_or(skin.bulge_height),
            env_map: env_map.unwrap_or(skin.env_map),
            jiggle_offsets: skin.jiggle_offsets,
        });
        let vertices = if ghoul2_gpu.is_some() {
            Arc::clone(&surface.vertices)
        } else {
            Arc::new(
                surface
                    .vertices
                    .iter()
                    .map(|vertex| DynamicModelVertex {
                        uv: match uv_xform {
                            Some(xform) => [vertex.uv[0] * xform[0] + xform[2], vertex.uv[1] * xform[1] + xform[3]],
                            None => vertex.uv,
                        },
                        color,
                        ..*vertex
                    })
                    .collect(),
            )
        };
        DynamicModelSurface {
            entity_num: surface.entity_num,
            wireframe_class: DynamicWireframeClass::Player,
            raster_visible: surface.raster_visible,
            vertices,
            indices: Arc::clone(&surface.indices),
            lighting_origin: surface.lighting_origin,
            rt_rigid: surface.rt_rigid.clone(),
            rt_skinned_key: surface.rt_skinned_key.as_ref().map(Arc::clone),
            ghoul2_gpu,
            fx_gpu_sprites: None,
            texture,
            alpha_mode,
        }
    }

    /// Minimal OpenJK-compatible vehicle presentation while the full vehicle
    /// animation/effects path is still being ported. Vehicle NPCs may send a
    /// `$name` through CS_MODELS/modelindex; resolve that through the winning
    /// ext_data/vehicles definition before registering the Ghoul2 model/skin.
    /// Genuine registration failures still use stock swoop as a visible fallback.
    fn present_vehicle_static(
        &mut self,
        entity: &PresentedEntity,
        game: &ClientGameState,
        current_time: i32,
        view: Option<Ghoul2PresentationView>,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let model_index = entity.state.field_i32("modelindex").unwrap_or(0);
        let requested = game
            .model_qpath(model_index)
            .ok_or_else(|| format!("vehicle modelindex {model_index} has no CS_MODELS entry"))?;
        let resolved = resolve_vehicle_model_request(&self.vehicle_definitions, &requested);
        let model = match resolved {
            Ok((requested_glm, requested_skin, requested_label)) => {
                match self.load_static_glm_in_game(
                    &requested_glm,
                    requested_skin.as_deref(),
                    &requested_label,
                ) {
                    Ok(model) => model,
                    Err(pending) if is_asset_pending(&pending) => return Ok(Vec::new()),
                    Err(primary_error) => self.vehicle_fallback(
                        entity.number,
                        &requested,
                        Some(&requested_glm),
                        requested_skin.as_deref(),
                        &primary_error,
                    )?,
                }
            }
            Err(primary_error) => {
                self.vehicle_fallback(entity.number, &requested, None, None, &primary_error)?
            }
        };

        let render_origin = ghoul2_render_origin(entity);
        let mut raster_visible = true;
        let vehicle_name = requested.strip_prefix('$').map(str::to_ascii_lowercase);
        let lod = if let Some(view) = view {
            if self.early_frustum_cull {
                self.perf.frustum_tests = self.perf.frustum_tests.saturating_add(1);
                let radius = ghoul2_entity_radius(entity) * ghoul2_model_scale(entity);
                if view.sphere_outside(render_origin, radius) {
                    self.perf.frustum_culled = self.perf.frustum_culled.saturating_add(1);
                    if !self.rt_shadow_casters_enabled {
                        return Ok(Vec::new());
                    }
                    raster_visible = false;
                }
            }
            let lod = ghoul2_lod_for_view(
                view,
                entity,
                render_origin,
                model.glm.lods.len(),
                self.lod_bias,
                self.lod_scale,
            );
            let bucket = lod.min(self.perf.lod_counts.len() - 1);
            self.perf.lod_counts[bucket] = self.perf.lod_counts[bucket].saturating_add(1);
            lod
        } else {
            self.perf.lod_counts[0] = self.perf.lod_counts[0].saturating_add(1);
            0
        };

        // With no bone animation commands set, Ghoul2Animator evaluates the
        // registered model in its base/static pose using OpenJK's Ghoul2 root
        // matrix. This is intentionally the minimum safe vehicle path;
        // vehicle-specific animation comes later.
        let pose_started = Instant::now();
        let pose = Ghoul2Animator::new(&model.gla).evaluate_pose_openjk_root(&model.gla, current_time)?;
        record_pose_eval(&mut self.perf, pose_started);
        let axis = angles_to_axis(entity.angles);
        let (axis, origin) = ghoul2_render_transform(entity, axis);
        if let Some(vehicle) = vehicle_name {
            self.vehicle_snaps.insert(
                entity.number,
                VehicleSnap { vehicle, model: Arc::clone(&model), pose: pose.clone(), axis, origin },
            );
        }
        let mut draws = self.render_glm_surfaces(
            entity.number,
            &requested,
            &model.glm,
            &model.gla,
            &model.surfaces,
            None,
            &pose,
            lod,
            axis,
            origin,
            [1.0, 1.0, 1.0, 1.0],
            None,
            true,
        )?;
        if !raster_visible {
            for surface in &mut draws {
                surface.raster_visible = false;
            }
        }
        Ok(draws)
    }

    /// See `viewer_foot_bolts`; `None` until the viewer's model has been posed.
    pub fn viewer_foot_bolts(&self) -> Option<[[f32; 3]; 2]> {
        self.viewer_foot_bolts
    }

    /// The viewer's animation input and lerp frames from its last posed frame.
    pub fn viewer_anim_debug(&self) -> Option<ViewerAnimDebug> {
        self.viewer_anim_debug
    }

    fn queue_dismember_smoke(
        &mut self,
        model: &PlayerModelAsset,
        pose: &[Matrix3x4],
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        tag: &str,
    ) -> Result<(), String> {
        let Some(matrix) = model_bolt_matrix_timed(
            &mut self.perf,
            &model.glm,
            &model.gla,
            pose,
            tag,
        )? else {
            return Ok(());
        };
        let smoke_origin = transform_jka_model_point(
            [matrix[0][3], matrix[1][3], matrix[2][3]],
            axis,
            origin,
        );
        // Public G2API_GetBoltMatrix's NEGATIVE_Y convention corresponds to
        // -column 1 of the internal bolt matrix used by this presenter.
        let smoke_dir = normalize_vec3(transform_jka_model_vector(
            [-matrix[0][1], -matrix[1][1], -matrix[2][1]],
            axis,
        ));
        self.fx_requests.push(PlayerFxRequest::EffectDir {
            name: "blaster/smoke_bolton".to_owned(),
            origin: smoke_origin,
            dir: smoke_dir,
        });
        Ok(())
    }

    fn present_detached_limb(
        &mut self,
        entity: &PresentedEntity,
        source_entity: Option<&PresentedEntity>,
        current_time: i32,
        view: Option<Ghoul2PresentationView>,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let level = self.ragdolls.dismemberment_level();
        let Some(part) = DismemberPart::from_model_part(entity.state.field_i32("modelGhoul2").unwrap_or(0)) else {
            return Ok(Vec::new());
        };
        if !part.allowed_at_level(level) {
            return Ok(Vec::new());
        }
        let Some(source_num) = dismember_source_entity(entity) else {
            return Ok(Vec::new());
        };
        let generation = dismember_generation(entity, source_num, part);
        let needs_visual = self
            .detached_limb_visuals
            .get(&entity.number)
            .is_none_or(|visual| visual.generation != generation);

        if needs_visual {
            let Some(source_entity) = source_entity else {
                return Ok(Vec::new());
            };
            let presented_torso_anim = self
                .entities
                .get(&source_num)
                .map(|runtime| runtime.animation.torso.animation_number);
            if !dismember_source_ready(source_entity, presented_torso_anim) {
                return Ok(Vec::new());
            }
            let Some(source) = self.dismember_source_snaps.get(&source_num).cloned() else {
                // OpenJK waits for a valid source Ghoul2/death pose too. The
                // next presentation frame will retry after the source is posed.
                return Ok(Vec::new());
            };
            let spec = part.spec();
            let surfaces = detached_limb_surface_set(
                &source.model,
                part,
                self.dismembered.get(&source_num),
            );
            if surfaces.is_empty() {
                return Err(format!(
                    "{} has no Ghoul2 subtree for dismember part {:?}",
                    source.model.key, part
                ));
            }

            let rotate_bone = if part == DismemberPart::Waist
                && model_bolt_matrix_timed(
                    &mut self.perf,
                    &source.model.glm,
                    &source.model.gla,
                    &source.pose,
                    spec.rotate_bone,
                )?
                .is_none()
            {
                // Exact OpenJK non-humanoid fallback.
                "pelvis"
            } else {
                spec.rotate_bone
            };
            let Some(bone_model) = model_bolt_matrix_timed(
                &mut self.perf,
                &source.model.glm,
                &source.model.gla,
                &source.pose,
                rotate_bone,
            )? else {
                return Err(format!("{} has no dismember bone {rotate_bone}", source.model.key));
            };
            let source_world = presentation_matrix(source.axis, source.origin);
            let spawn_body_matrix = multiply_3x4(
                &source_world,
                &scaled_bone_matrix(bone_model, source.model_scale),
            );
            let spawn_entity_matrix = source_world;

            let capsule_local = if let Some(end_name) = spec.collider_b {
                let endpoint_world = |presenter: &mut PlayerPresenter, bone: &str| -> Result<Option<[f32; 3]>, String> {
                    Ok(model_bolt_matrix_timed(
                        &mut presenter.perf,
                        &source.model.glm,
                        &source.model.gla,
                        &source.pose,
                        bone,
                    )?
                    .map(|matrix| {
                        let matrix = scaled_bone_matrix(matrix, source.model_scale);
                        affine_point(&source_world, [matrix[0][3], matrix[1][3], matrix[2][3]])
                    }))
                };
                let a = endpoint_world(self, spec.collider_a)?;
                let b = endpoint_world(self, end_name)?;
                match (a, b, affine_inverse_3x4(&spawn_body_matrix)) {
                    (Some(a), Some(b), Some(inverse)) => Some((
                        affine_point(&inverse, a),
                        affine_point(&inverse, b),
                    )),
                    _ => None,
                }
            } else {
                None
            };

            let source_origin = source_entity.origin;
            let outward = normalize_vec3([
                entity.origin[0] - source_origin[0],
                entity.origin[1] - source_origin[1],
                entity.origin[2] - source_origin[2],
            ]);
            let network_velocity = super::entity_vec3(&source_entity.state, "pos.trDelta")
                .unwrap_or([0.0; 3]);
            let inherited = self
                .ragdolls
                .ragdoll_bone_velocity(source_num, &source.model.gla, rotate_bone)
                .unwrap_or(network_velocity);
            // Server G_Dismember adds an 80 u/s outward kick before optional
            // saber-swing contribution. We can observe the source trajectory but
            // not the server's private saber swing vector, so preserve the exact
            // visible component and let Rapier take over from there.
            let mut linear_velocity = [
                inherited[0] + outward[0] * 80.0,
                inherited[1] + outward[1] * 80.0,
                inherited[2] + outward[2] * 80.0,
            ];
            if matches!(part, DismemberPart::Head | DismemberPart::Waist) {
                // Exact server G_Dismember baseline before optional saber-swing
                // contribution.
                linear_velocity[2] += 10.0;
            }
            let mut spin_axis = [outward[1], -outward[0], 0.35];
            spin_axis = normalize_vec3(spin_axis);
            let spin = 2.6 + f32::from((entity.number % 5) as u8) * 0.35;
            let angular_velocity = [spin_axis[0] * spin, spin_axis[1] * spin, spin_axis[2] * spin];

            // Stock CG_General emits one blaster/smoke_bolton puff from both
            // exposed cap tags when the sever is created. The detached cap then
            // repeats every 400 ms while the server part is moving.
            self.queue_dismember_smoke(
                &source.model,
                &source.pose,
                source.axis,
                source.origin,
                spec.limb_tag,
            )?;
            self.queue_dismember_smoke(
                &source.model,
                &source.pose,
                source.axis,
                source.origin,
                spec.stub_tag,
            )?;

            self.ragdolls.ensure_detached_limb(
                DetachedLimbSpawn {
                    entity: entity.number,
                    generation,
                    body_world: spawn_body_matrix,
                    capsule_local,
                    radius: spec.radius * source.model_scale,
                    mass_kg: spec.mass_kg * source.model_scale.powi(3),
                    linear_velocity,
                    angular_velocity,
                },
                current_time,
            );

            self.detached_limb_visuals.insert(
                entity.number,
                DetachedLimbVisual {
                    generation,
                    source_entity: source_num,
                    part,
                    model: Arc::clone(&source.model),
                    pose: source.pose,
                    surfaces,
                    spawn_entity_matrix,
                    spawn_body_matrix,
                    body_rgba: source.body_rgba,
                    body_rgb: source.body_rgb,
                    model1_weapon: source.model1_weapon,
                    model1_primary_saber: source.model1_primary_saber,
                    client_info: source.client_info,
                    next_smoke_time: current_time.saturating_add(400),
                },
            );
        }

        let Some(visual) = self.detached_limb_visuals.get(&entity.number).cloned() else {
            return Ok(Vec::new());
        };
        if visual.generation != generation {
            return Ok(Vec::new());
        }

        // Rapier renders at display rate from the fixed-step interpolation. If
        // client physics is off/unavailable, preserve OpenJK's server ExPhys
        // translation while keeping the sever pose rigid.
        let current_body = self
            .ragdolls
            .detached_limb_matrix(entity.number, generation)
            .unwrap_or_else(|| {
                let mut matrix = visual.spawn_body_matrix;
                matrix[0][3] = entity.origin[0];
                matrix[1][3] = entity.origin[1];
                matrix[2][3] = entity.origin[2];
                matrix
            });
        let render_matrix = affine_inverse_3x4(&visual.spawn_body_matrix)
            .map(|inverse| multiply_3x4(&multiply_3x4(&current_body, &inverse), &visual.spawn_entity_matrix))
            .unwrap_or(visual.spawn_entity_matrix);
        let (axis, origin) = presentation_axis_origin(&render_matrix);
        let lod = view.map_or(0, |view| {
            ghoul2_lod_for_view(
                view,
                entity,
                origin,
                visual.model.glm.lods.len(),
                self.lod_bias,
                self.lod_scale,
            )
        });
        let mut draws = self.render_glm_surfaces_tinted_selected(
            entity.number,
            &format!("{}#dismember-{:?}", visual.model.key, visual.part),
            &visual.model.glm,
            &visual.model.gla,
            &visual.model.surfaces,
            None,
            &visual.pose,
            lod,
            axis,
            origin,
            visual.body_rgba,
            visual.body_rgb,
            None,
            true,
            Some(&visual.surfaces),
        )?;
        // Detached pieces are one rigid caster, not another articulated skin.
        // CPU and GPU skinning both retain the frozen sever pose; the entity
        // transform above supplies all subsequent movement.
        for draw in &mut draws {
            draw.lighting_origin = Some(origin);
        }

        // CG_General duplicates Ghoul2 after removing model slot 2/3 from the
        // owner, so a held model-slot-1 weapon/hilt remains attached to the
        // detached copy. Generic limb entities never run CG_AddSaberBlade, so
        // saber hilts are rendered here without live blades/trails.
        if visual.model1_primary_saber {
            if let Err(error) = self.append_player_sabers(
                &mut draws,
                entity,
                &visual.client_info,
                &visual.model,
                &visual.pose,
                axis,
                origin,
                current_time,
                visual.body_rgba[3],
                [true, false],
                false,
            ) {
                self.report_saber_warning_once(entity.number, &format!("dismember saber hilt: {error}"));
            }
        } else if let Some(weapon) = visual.model1_weapon {
            if let Err(error) = self.append_held_weapon(
                &mut draws,
                entity,
                weapon,
                &visual.model,
                &visual.pose,
                axis,
                origin,
                current_time,
                visual.body_rgba[3],
            ) {
                self.report_saber_warning_once(entity.number, &format!("dismember held weapon {weapon}: {error}"));
            }
        }

        let tr_delta = super::entity_vec3(&entity.state, "pos.trDelta").unwrap_or([0.0; 3]);
        let moving = tr_delta.iter().any(|value| value.abs() > f32::EPSILON);
        if moving && current_time > visual.next_smoke_time {
            let spec = visual.part.spec();
            self.queue_dismember_smoke(
                &visual.model,
                &visual.pose,
                axis,
                origin,
                spec.limb_tag,
            )?;
            if let Some(live) = self.detached_limb_visuals.get_mut(&entity.number) {
                if live.generation == generation {
                    live.next_smoke_time = current_time.saturating_add(400);
                }
            }
        }
        Ok(draws)
    }

    pub fn present_snapshot_players(
        &mut self,
        entities: &[PresentedEntity],
        game: &ClientGameState,
        siege_classes: &[jka_assets::siege::SiegeClassVisual],
        current_time: i32,
        preserve_entity: Option<u16>,
        hidden_first_person_entity: Option<u16>,
        view: Option<Ghoul2PresentationView>,
        forced_models: Option<&ForcedPlayerModels>,
    ) -> Vec<DynamicModelSurface> {
        self.stage_time_ms = current_time;
        self.japro_cinfo2 = game.japro_cinfo2();
        self.cloth.begin_frame(current_time);
        self.stage_view_position = view.map(|view| view.position.to_array());
        if let Some(snapshot) = game.current_snapshot() {
            let viewer_client = snapshot.player_state.field_i32("clientNum").unwrap_or(-1);
            if viewer_client != self.viewer_client {
                self.viewer_foot_bolts = None;
                self.viewer_anim_debug = None;
            }
            self.viewer_client = viewer_client;
            self.viewer_dueling = snapshot.player_state.field_i32("duelInProgress").unwrap_or(0) != 0;
            self.viewer_style = viewer_style_from(&snapshot.player_state, game.is_japro(), self.japro.style_player);
        }
        // Keep the disabled path effectively free: only parse the viewer's
        // ClientInfo when a force-model override is actually enabled.
        let viewer_info = forced_models.and_then(|_| {
            usize::try_from(self.viewer_client)
                .ok()
                .and_then(|client| game.client_info(client, siege_classes))
        });
        let mut draws = Vec::new();
        let mut live_entities = HashSet::new();
        let mut live_ragdolls = HashSet::new();
        let mut live_detached_limbs = HashSet::new();
        let mut detached_entities = Vec::new();
        if let Some(entity_num) = preserve_entity {
            // The local/followed player is presented separately just like
            // OpenJK's predictedPlayerEntity, but it still owns persistent
            // centity/playerEntity state across frames.
            live_entities.insert(entity_num);
        }
        live_entities.extend(self.external_live_entities.iter().copied());
        // CG_Player resolves lookTarget against cg_entities before BG_G2PlayerAngles.
        let entity_origins = entities
            .iter()
            .map(|entity| (entity.number, entity.origin))
            .collect::<HashMap<_, _>>();
        self.duel_shell_gray = self.duel_shell_brightness(game, &entity_origins, preserve_entity);

        // Authoritative dismemberment is transmitted as ET_GENERAL/G2_MODEL_PART.
        // Apply the source surface mutation before CG_Player draws the corpse,
        // then draw the detached copies after source poses have been evaluated.
        let dismemberment_level = self.ragdolls.dismemberment_level();
        if dismemberment_level == 0 {
            self.dismembered.clear();
            self.dismember_source_snaps.clear();
            self.detached_limb_visuals.clear();
        } else {
            // OpenJK CG_ReattachLimb restores a living client instance.
            for entity in entities.iter().filter(|entity| entity.entity_type == ET_PLAYER || entity.entity_type == ET_NPC) {
                if entity.state.field_i32("eFlags").unwrap_or(0) & EF_DEAD == 0 {
                    self.dismembered.remove(&entity.number);
                }
            }
            for entity in entities {
                let Some(source) = dismember_source_entity(entity) else { continue };
                let Some(part) = DismemberPart::from_model_part(entity.state.field_i32("modelGhoul2").unwrap_or(0)) else { continue };
                if !part.allowed_at_level(dismemberment_level) {
                    continue;
                }
                let Some(source_entity) = entities.iter().find(|candidate| candidate.number == source) else {
                    continue;
                };
                // Match CG_General's creation gate before mutating the source
                // surfaces. At this prepass point the cached torso animation is
                // from the last presented frame; if no runtime exists yet, wait
                // rather than cutting a source before its death pose is valid.
                let presented_torso_anim = self
                    .entities
                    .get(&source)
                    .map(|runtime| runtime.animation.torso.animation_number);
                if !dismember_source_ready(source_entity, presented_torso_anim) {
                    continue;
                }
                self.dismembered.entry(source).or_default().insert(part);
                live_detached_limbs.insert(entity.number);
                detached_entities.push(entity);
            }
        }

        // ET_PLAYER uses CG_Player and ET_NPC uses CG_G2Animated. ET_BODY is
        // CG_General in OpenJK; this Rust presenter still owns the shared body
        // mesh/ragdoll submission, but its weapon state comes only from ircg.
        // NPCs resolve `ci = cent->npcClient` instead of cgs.clientinfo[].
        let intermission = game.rendering_intermission();
        for entity in entities
            .iter()
            .filter(|entity| {
                entity.entity_type == ET_PLAYER
                    || entity.entity_type == ET_NPC
                    || entity.entity_type == ET_BODY
            })
        {
            // Hidden intermission entities remain live centities in TaystJK;
            // only their CG_AddCEntity presentation is skipped. Preserve the
            // cached Ghoul2 owner while withholding its draw/update work.
            live_entities.insert(entity.number);
            // TaystJK CG_AddCEntity suppresses ET_PLAYER outright during
            // intermission and suppresses ET_NPC only for vehicles. ET_BODY
            // intentionally remains eligible, matching the original switch.
            if suppressed_during_intermission(intermission, entity) {
                continue;
            }
            let entity_eflags = entity.state.field_i32("eFlags").unwrap_or(0);
            if entity.entity_type == ET_BODY
                || entity_eflags & (EF_DEAD | EF_RAG) != 0
                || self.force_gripped_entities.contains(&entity.number)
            {
                live_ragdolls.insert(entity.number);
            }

            // OpenJK CG_AddPacketEntities first synthesizes/adds the predicted
            // player from playerState, then explicitly skips that same client
            // number while walking snapshot entities. `preserve_entity` is our
            // separately-presented local/followed player, so presenting the
            // snapshot copy here as well queues a second SaberBlade for the same
            // (entity,saber,blade) key. If the two poses disagree about wall
            // contact, the later copy clears haveOldPos every frame and saber
            // marks can never connect. Keep it live, but do not present it twice.
            if preserve_entity == Some(entity.number) {
                continue;
            }

            // CG_Player returns before animating or submitting anything for
            // EF_NODRAW (e.g. NPCs a spawner is still holding, parked with
            // anim 0) and EF2_SHIP_DEATH; the centity itself stays alive.
            if entity.state.field_i32("eFlags").unwrap_or(0) & EF_NODRAW != 0
                || entity.state.field_i32("eFlags2").unwrap_or(0) & EF2_SHIP_DEATH != 0
            {
                continue;
            }
            self.look = crate::japro_cg::Appearance::NORMAL;
            if entity.entity_type == ET_PLAYER {
                self.look = crate::japro_cg::classify(
                    &self.viewer_style,
                    i32::from(entity.number),
                    entity.state.field_i32("bolt1").unwrap_or(0),
                    entity_eflags & EF_DEAD != 0,
                );
                if !self.look.visible {
                    // CG_Player returns before animating, shadowing or adding
                    // sprites: dueling/racing bystanders simply do not exist.
                    continue;
                }
            } else if entity.entity_type == ET_BODY && self.viewer_style.dueling && self.viewer_style.japro {
                // CG_General hides corpses from a jaPRO duelist's view.
                continue;
            }
            if entity.entity_type != ET_BODY {
                self.queue_blob_shadow(entity);
            }
            if entity.entity_type == ET_NPC
                && entity.state.field_i32("NPC_class").unwrap_or(0) == crate::cgame::CLASS_VEHICLE
            {
                match self.present_vehicle_static(entity, game, current_time, view) {
                    Ok(mut vehicle_draws) => {
                        self.report_player_status(
                            entity.number,
                            format!("vehicle submitted={}", usize::from(!vehicle_draws.is_empty())),
                        );
                        draws.append(&mut vehicle_draws);
                    }
                    Err(error) => self.report_player_status(
                        entity.number,
                        format!("vehicle submitted=0 reason={error}"),
                    ),
                }
                continue;
            }

            let (client_num, info) = if entity.entity_type == ET_NPC {
                match game.npc_client_info(&entity.state) {
                    Ok(info) => (usize::from(entity.number), info),
                    Err(reason) => {
                        self.report_player_status(entity.number, format!("npc submitted=0 reason={reason}"));
                        continue;
                    }
                }
            } else {
                let client_num = entity
                    .state
                    .field_i32("clientNum")
                    .unwrap_or(i32::from(entity.number));
                let Ok(client_num) = usize::try_from(client_num) else {
                    self.report_player_status(entity.number, format!("clientNum={client_num} submitted=0 reason=invalid-client"));
                    continue;
                };
                let Some(mut info) = game.client_info(client_num, siege_classes) else {
                    self.report_player_status(entity.number, format!("clientNum={client_num} submitted=0 reason=missing-client-info"));
                    continue;
                };
                if let Some(forced) = forced_models {
                    forced.apply_to(&mut info, viewer_info.as_ref());
                    // TaystJK applies Siege class forcedModel/forcedSkin after
                    // cg_forceModel, so class-required visuals remain authoritative.
                    if info.gametype == GT_SIEGE {
                        if let Some(class) = find_siege_class_visual(siege_classes, &info.siege_class) {
                            if !class.forced_model.is_empty() {
                                info.model_name.clone_from(&class.forced_model);
                            }
                            if !class.forced_skin.is_empty() {
                                info.skin_name.clone_from(&class.forced_skin);
                            }
                        }
                    }
                }
                (client_num, info)
            };

            let look_target_origin = openjk_look_target_origin(entity, &entity_origins);
            self.queue_player_sprites(entity, game, current_time, hidden_first_person_entity != Some(entity.number));
            match self.present_player(
                entity,
                &info,
                current_time,
                1.0,
                look_target_origin,
                false,
                hidden_first_person_entity != Some(entity.number),
                false,
                view,
            ) {
                Ok(mut player_draws) => draws.append(&mut player_draws),
                Err(error) => {
                    self.report_player_status(entity.number, format!(
                        "clientNum={client_num} requested={}/{} submitted=0 reason={error}",
                        info.model_name, info.skin_name,
                    ));
                }
            }

            self.look = crate::japro_cg::Appearance::NORMAL;
            if entity.entity_type != ET_BODY {
                match self.present_thrown_saber_for_player(
                    entity,
                    &info,
                    entities,
                    game,
                    current_time,
                    1.0,
                ) {
                    Ok(mut saber_draws) => draws.append(&mut saber_draws),
                    Err(error) => self.report_saber_warning_once(entity.number, &error),
                }
            }
        }

        // G2_MODEL_PART is created by the server, but its old ExPhys motion is
        // only 20 Hz. Clone the exact posed Ghoul2 subtree and hand one compact
        // rigid body to Rapier; the server entity still decides that the sever
        // happened and which part it was.
        for limb in detached_entities {
            let source_num = dismember_source_entity(limb);
            let source = source_num.and_then(|source| entities.iter().find(|entity| entity.number == source));
            match self.present_detached_limb(limb, source, current_time, view) {
                Ok(mut limb_draws) => draws.append(&mut limb_draws),
                Err(error) => self.report_player_status(limb.number, format!("dismember submitted=0 reason={error}")),
            }
        }
        self.ragdolls.retain_detached_limbs(&live_detached_limbs);
        self.detached_limb_visuals
            .retain(|entity, _| live_detached_limbs.contains(entity));

        // The followed/local player may not exist in packet entities but can
        // still be an exact forceGripCripple victim. Keep that live ragdoll
        // until present_player_entity runs later in the frame.
        live_ragdolls.extend(self.force_gripped_entities.iter().copied());
        live_ragdolls.extend(self.active_impulse_ragdolls.keys().copied());
        self.entities.retain(|entity_num, _| live_entities.contains(entity_num));
        self.jiggle.retain_entities(&live_entities);
        let mut live_dismember_sources = live_entities.clone();
        live_dismember_sources.extend(self.detached_limb_visuals.values().map(|visual| visual.source_entity));
        self.dismember_source_snaps
            .retain(|entity_num, _| live_dismember_sources.contains(entity_num));
        self.dismembered.retain(|entity_num, _| live_dismember_sources.contains(entity_num));
        self.ragdolls.retain_visible(&live_ragdolls);
        self.thrown_sabers.retain(|entity_num, _| live_entities.contains(entity_num));
        self.player_diagnostics.retain(|entity_num, _| live_entities.contains(entity_num));
        if !self.logged_first_draw && !draws.is_empty() {
            devprintln!(
                1,
                "PLAYER DRAW READY: players={} surfaces={}",
                live_entities.len(),
                draws.len(),
            );
            self.logged_first_draw = true;
        }
        draws
    }

    /// Present one local/followed player through the same Ghoul2/model/animation
    /// path used for snapshot players. This is deliberately not a second
    /// third-person renderer.
    pub fn present_player_entity(
        &mut self,
        entity: &PresentedEntity,
        info: &crate::cgame::ClientInfo,
        current_time: i32,
        entity_alpha: f32,
        look_target_origin: Option<[f32; 3]>,
        force_reset: bool,
        submit_geometry: bool,
        first_person_saber: bool,
        view: Option<Ghoul2PresentationView>,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let mut entity_alpha = entity_alpha;
        self.look = crate::japro_cg::Appearance::NORMAL;
        if entity.entity_type == ET_BODY {
            if let Some(&started) = self.body_fade.get(&entity.number) {
                // CG_Player: alpha = 254 - elapsed * 0.08, invisible below 1.
                let byte = 254.0 - (current_time - started).max(0) as f32 * 0.08;
                if byte < 1.0 {
                    return Ok(Vec::new());
                }
                entity_alpha *= byte / 255.0;
            }
        }
        if entity.entity_type != ET_BODY
            && entity.state.field_i32("eFlags").unwrap_or(0) & EF_NODRAW == 0
            && entity.state.field_i32("eFlags2").unwrap_or(0) & EF2_SHIP_DEATH == 0
        {
            self.queue_blob_shadow(entity);
        }
        if entity.entity_type == ET_PLAYER && i32::from(entity.number) == self.viewer_client {
            self.viewer_anim_debug = None;
        }
        let result = self.present_player(
            entity,
            info,
            current_time,
            entity_alpha,
            look_target_origin,
            force_reset,
            submit_geometry,
            first_person_saber,
            view,
        );
        if entity.entity_type == ET_PLAYER && i32::from(entity.number) == self.viewer_client {
            let surfaces = result.as_ref().map_or(0, |draws| draws.len() as u32);
            let debug = self.viewer_anim_debug.get_or_insert_with(ViewerAnimDebug::default);
            debug.call_time = current_time;
            debug.submit_geometry = submit_geometry;
            debug.surfaces = surfaces;
        }
        result
    }

    /// Present a visual-only race ghost through the same player model/animation
    /// path without player sprites, blob shadows, gameplay classification, or RT
    /// shadow casting. Geometry still uses the normal depth test and view culling.
    pub fn present_race_ghost_entity(
        &mut self,
        entity: &PresentedEntity,
        info: &crate::cgame::ClientInfo,
        current_time: i32,
        entity_alpha: f32,
        force_reset: bool,
        view: Option<Ghoul2PresentationView>,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let previous_look = self.look;
        self.look = crate::japro_cg::Appearance::NORMAL;
        let footstep_count = self.footsteps.len();
        let fx_request_count = self.fx_requests.len();
        let footstep_stages = self.footstep_stages;
        self.footstep_stages = FootstepStages { sounds: false, effects: false, marks: false };
        let result = self.present_player(
            entity,
            info,
            current_time,
            entity_alpha.clamp(0.02, 1.0),
            None,
            force_reset,
            true,
            false,
            view,
        );
        self.look = previous_look;
        // The ghost is deliberately silent and visual-only. Do not even run
        // footstep ground traces, and let no animation/force FX escape to the
        // shared event/audio/WeaponFx queues. Cosmetic model draws are retained.
        self.footstep_stages = footstep_stages;
        self.footsteps.truncate(footstep_count);
        self.fx_requests.truncate(fx_request_count);
        let mut draws = result?;
        for draw in &mut draws {
            draw.rt_rigid = None;
            draw.rt_skinned_key = None;
        }
        Ok(draws)
    }

    fn present_player(
        &mut self,
        entity: &PresentedEntity,
        info: &crate::cgame::ClientInfo,
        current_time: i32,
        entity_alpha: f32,
        look_target_origin: Option<[f32; 3]>,
        force_reset: bool,
        submit_geometry: bool,
        first_person_saber: bool,
        view: Option<Ghoul2PresentationView>,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        self.poll_asset_completions();

        // CG_NewClientInfo/CG_RegisterClientModelname normalize team presentation
        // before model registration. Do the same here after any cg_forceModel
        // rewrite, so model caching and async requests see the final team skin.
        let mut team_info = info.clone();
        if team_info.gametype >= crate::cgame::GT_TEAM
            && team_info.gametype != crate::cgame::GT_SIEGE
            && !team_info.jedi_v_merc
        {
            team_info.team_color_override = crate::cgame::validate_skin_for_team(
                &team_info.model_name,
                &mut team_info.skin_name,
                team_info.team,
                |qpath| self.assets.contains_qpath(qpath),
            );
        } else {
            team_info.team_color_override = None;
        }
        let info = &team_info;

        let cached_model = self
            .entities
            .get(&entity.number)
            .filter(|runtime| {
                runtime.model_settled
                    && runtime.requested_model_name.eq_ignore_ascii_case(&info.model_name)
                    && runtime.requested_skin_name.eq_ignore_ascii_case(&info.skin_name)
            })
            .map(|runtime| Arc::clone(&runtime.model));
        let (model, model_settled) = if let Some(model) = cached_model {
            (model, true)
        } else if !self.async_loading_enabled() {
            (self.load_model(info)?, true)
        } else {
            // Request the real model immediately but never wait for it: keep
            // the entity's previous model, or the resident default body for a
            // new player, until the worker finishes; then swap automatically.
            match self.resolve_player_model_async(info) {
                ModelResolution::Ready(model) => (model, true),
                ModelResolution::Provisional(model) => {
                    match self.entities.get(&entity.number) {
                        Some(runtime) => (Arc::clone(&runtime.model), false),
                        None => (model, false),
                    }
                }
                ModelResolution::Loading => match self.entities.get(&entity.number) {
                    Some(runtime) => (Arc::clone(&runtime.model), false),
                    None => return Ok(Vec::new()),
                },
                ModelResolution::Failed(error) => match self.entities.get(&entity.number) {
                    // Keep showing the last valid model rather than vanishing.
                    Some(runtime) => (Arc::clone(&runtime.model), true),
                    None => return Err(error),
                },
            }
        };

        // OpenJK loads a client's selected saber definitions/models as part of
        // client/Ghoul2 setup, not only on the first frame the saber happens to
        // be visible. Keep the assets cached up front and let currentState.weapon
        // decide whether they are actually attached/rendered this frame. Server
        // weapon-disable rules affect what the server grants; they must not hide
        // a weapon that is explicitly present in a snapshot.
        self.register_client_saber_assets(info, entity.number);

        let mut recreate = self
            .entities
            .get(&entity.number)
            .is_none_or(|runtime| runtime.model_key != model.key);
        if recreate {
            // A model swap onto the very same skeleton (all humanoid players
            // share one GLA through the asset workers) keeps the running
            // animation/angle state instead of restarting the player.
            if let Some(runtime) = self
                .entities
                .get_mut(&entity.number)
                .filter(|runtime| Arc::ptr_eq(&runtime.model.gla, &model.gla))
            {
                runtime.model_key.clone_from(&model.key);
                runtime.model = Arc::clone(&model);
                recreate = false;
            }
        }
        if recreate {
            let animation = PlayerAnimationState::new(&model.gla, current_time)?;
            self.entities.insert(
                entity.number,
                EntityPlayerState {
                    model_key: model.key.clone(),
                    model_settled,
                    requested_model_name: info.model_name.clone(),
                    requested_skin_name: info.skin_name.clone(),
                    model: Arc::clone(&model),
                    animation,
                    player_angles: PlayerAngleState::reset_from_angles(entity.angles),
                    last_angle_time: current_time,
                    last_e_flags: entity.state.field_i32("eFlags").unwrap_or(0),
                    last_client_num: entity
                        .state
                        .field_i32("clientNum")
                        .unwrap_or(i32::from(entity.number)),
                    ghoul2_weapon: None,
                    cent_weapon: 0,
                    attached_weapon: None,
                    primary_saber_attached: false,
                    secondary_saber_attached: false,
                    saber_names: [info.saber_name.clone(), info.saber2_name.clone()],
                    bone_smooth_history: None,
                    bone_smooth_time: current_time,
                },
            );
        }

        let body_copy = if entity.entity_type == ET_BODY {
            self.body_queue_copies
                .get(&entity.number)
                .copied()
                .filter(|copy| usize::from(copy.source_client) == info.client_num)
        } else {
            None
        };

        let e_flags = entity.state.field_i32("eFlags").unwrap_or(0);
        let client_num = entity
            .state
            .field_i32("clientNum")
            .unwrap_or(i32::from(entity.number));
        let runtime = self
            .entities
            .get_mut(&entity.number)
            .ok_or_else(|| "player animation state disappeared".to_owned())?;
        if runtime.model_settled != model_settled
            || !runtime.requested_model_name.eq_ignore_ascii_case(&info.model_name)
            || !runtime.requested_skin_name.eq_ignore_ascii_case(&info.skin_name)
        {
            runtime.model_settled = model_settled;
            runtime.requested_model_name.clone_from(&info.model_name);
            runtime.requested_skin_name.clone_from(&info.skin_name);
        }
        let previous_e_flags = runtime.last_e_flags;
        let time_rewound = current_time < runtime.last_angle_time;
        // CG_SetInitialSnapshot calls CG_ResetEntity for every entity, and
        // CG_TransitionEntity does the same whenever interpolation breaks. A
        // newly-created Rust runtime is the initial-snapshot/PVS equivalent.
        let reset_player_entity = recreate
            || openjk_player_entity_needs_reset(
                previous_e_flags,
                runtime.last_client_num,
                e_flags,
                client_num,
                force_reset,
                time_rewound,
            );
        if reset_player_entity {
            // OpenJK CG_TransitionEntity -> CG_ResetEntity ->
            // CG_ResetPlayerEntity when interpolation is broken.  This resets
            // the lerp frames and the persistent torso/legs angle swing state.
            runtime.animation.reset_player_entity();
            // OpenJK CG_ResetPlayerEntity clears/reseeds cent->pe torso/legs
            // swing state, but preserves clientInfo corrTime/lookTime/
            // lastHeadAngles. Only superSmoothTime is reset there.
            runtime.player_angles.reset_entity_swing_from_angles(entity.angles);
            // OpenJK does not zero cg.frametime on ordinary entity
            // teleports/resets. Only guard an actual local time rewind
            // from producing a negative/huge bridge frametime.
            if time_rewound {
                runtime.last_angle_time = current_time;
            }
        }
        runtime.last_e_flags = e_flags;
        runtime.last_client_num = client_num;

        let weapon = entity.state.field_i32("weapon").unwrap_or(0);
        // WP_SetSaber: "none"/"remove" and the two-handed rules decide which
        // clientInfo_t::saber[] slots actually hold a saber.
        let [has_primary_saber, has_secondary_saber] = self
            .saber_definitions
            .equipped_slots([&info.saber_name, &info.saber2_name], info.client_num < MAX_CLIENTS);
        // CG_NewClientInfo: when a saber name changes the new saberInfo_t and
        // Ghoul2 hilt instance replace the old ones, and the centity's weapon
        // bookkeeping is cleared ("force a refresh") so the next CG_Player /
        // CG_CheckPlayerG2Weapons recopies BOTH hilts and relights the blades.
        // Without this a second saber selected at runtime (`saber kyle kyle`)
        // stays unattached until the primary is thrown and caught.
        if entity.entity_type != ET_BODY {
            for slot in 0..2 {
                let name = if slot == 0 { &info.saber_name } else { &info.saber2_name };
                if runtime.saber_names[slot].eq_ignore_ascii_case(name) {
                    continue;
                }
                runtime.saber_names[slot].clone_from(name);
                runtime.ghoul2_weapon = None;
                runtime.cent_weapon = 0;
                // The replacement saberInfo_t starts with zero-length blades
                // that extend gradually toward their desired length.
                for blade in 0..MAX_SABER_BLADES {
                    self.saber_blade_lengths.insert(
                        SaberBladeLengthKey {
                            client_num: info.client_num,
                            saber_num: slot as u8,
                            blade_num: blade as u8,
                        },
                        SaberBladeLengthState::new(
                            DEFAULT_SABER_BLADE_LENGTH_MAX,
                            0.0,
                            current_time,
                        ),
                    );
                }
            }
        }
        if entity.entity_type == ET_BODY {
            // ET_BODY is CG_General in OpenJK. Never derive its held models from
            // currentState.weapon: `ircg` already captured the duplicated G2
            // model slots and CG_BodyQueueCopy's knownWeapon correction.
            runtime.ghoul2_weapon = None;
            runtime.cent_weapon = 0;
            if let Some(copy) = body_copy {
                // CG_BodyQueueCopy operates on an ET_BODY destination. Its
                // low-tier model-1 replacement therefore goes through
                // CG_G2WeaponInstance(body, knownWeapon): WP_SABER is the
                // default g2WeaponInstances saber, not the client's custom
                // primary hilt. Model index 2 is not replaced and can retain
                // the copied custom second saber.
                runtime.attached_weapon = copy.model1_weapon;
                runtime.primary_saber_attached = false;
                runtime.secondary_saber_attached = copy.model2_saber;
            } else {
                runtime.attached_weapon = None;
                runtime.primary_saber_attached = false;
                runtime.secondary_saber_attached = false;
            }
        } else {
            // CG_ResetPlayerEntity performs this remote-saber copy before the
            // ordinary CG_Player weapon update. In particular, if saber 0 is
            // already in flight when a remote player first appears, this copy
            // creates both model index 1 and model index 2; the later flight
            // path removes only index 1, leaving the second saber in hand.
            if reset_player_entity {
                reset_remote_player_saber_attachment(
                    &mut runtime.ghoul2_weapon,
                    &mut runtime.cent_weapon,
                    &mut runtime.attached_weapon,
                    &mut runtime.primary_saber_attached,
                    &mut runtime.secondary_saber_attached,
                    weapon,
                    i32::from(entity.number) != self.viewer_client,
                    has_primary_saber,
                    has_secondary_saber,
                );
            }

            let dead = e_flags & EF_DEAD != 0;
            let instance = if weapon == WP_SABER && has_primary_saber {
                Some(WP_SABER)
            } else {
                weapon_world_model(weapon).map(|_| weapon)
            };
            update_weapon_attachment(
                &mut runtime.ghoul2_weapon,
                &mut runtime.cent_weapon,
                &mut runtime.attached_weapon,
                &mut runtime.primary_saber_attached,
                &mut runtime.secondary_saber_attached,
                weapon,
                instance,
                entity.state.field_i32("saberInFlight").unwrap_or(0) != 0,
                dead,
                info.team == TEAM_SPECTATOR && entity.entity_type == ET_PLAYER,
                has_primary_saber,
                has_secondary_saber,
            );

            // CG_Player's saber presentation removes model index 1 while the
            // primary saber is in flight and copies it back when held again.
            // This is model-slot state, separate from whether blades are lit.
            if weapon == WP_SABER && runtime.cent_weapon == WP_SABER && has_primary_saber {
                if entity.state.field_i32("saberInFlight").unwrap_or(0) != 0 {
                    runtime.primary_saber_attached = false;
                } else {
                    runtime.attached_weapon = None;
                    runtime.primary_saber_attached = true;
                }
            }
        }
        // Preserve model slot 1 before applying CG_General's source-instance
        // mutations. The detached Ghoul2 copy is made after slot 2/3 are
        // removed but before slot 1 is removed for weapon-arm/waist severs.
        let detached_model1_weapon = runtime.attached_weapon;
        let detached_model1_primary_saber = runtime.primary_saber_attached;
        let mut attached_weapon = runtime.attached_weapon;
        let mut attached_sabers = [runtime.primary_saber_attached, runtime.secondary_saber_attached];
        if let Some(parts) = self.dismembered.get(&entity.number) {
            // OpenJK removes Ghoul2 model index 2 from the source before every
            // limb duplicate (and model index 3/jetpack as well). The duplicate
            // therefore never contains the second saber.
            attached_sabers[1] = false;
            // After duplicating, model index 1 is removed from the source only
            // when its weapon-holding subtree was actually severed.
            if parts.iter().copied().any(DismemberPart::removes_weapon) {
                attached_weapon = None;
                attached_sabers[0] = false;
            }
        }

        // OpenJK renders ET_BODY through CG_General using the Ghoul2 instance
        // copied at respawn. Our renderer still owns the equivalent corpse pose
        // and Rapier handoff here; once ragging, preserve the existing yaw-only
        // corpse orientation used by this port.
        runtime.animation.animator.clear_bone_angle_overrides();
        // Match OpenJK CG_G2PlayerAngles/CG_G2Animated: dead entities and
        // explicit EF_RAG entities enter the ragdoll seam regardless of whether
        // they have already been copied into the ET_BODY queue. This is crucial
        // for NPCs, which can remain ET_NPC while dead.
        let corpse_ragdoll_requested =
            entity.entity_type == ET_BODY || e_flags & (EF_DEAD | EF_RAG) != 0;
        let force_grip_ragdoll_requested =
            !corpse_ragdoll_requested && self.force_gripped_entities.contains(&entity.number);
        let impulse_ragdoll = (!corpse_ragdoll_requested && !force_grip_ragdoll_requested)
            .then(|| self.active_impulse_ragdolls.get(&entity.number).copied())
            .flatten()
            .filter(|impulse| current_time < impulse.expires_at);
        let impulse_ragdoll_requested = impulse_ragdoll.is_some();
        let ragdoll_requested =
            corpse_ragdoll_requested || force_grip_ragdoll_requested || impulse_ragdoll_requested;
        // OpenJK's corpse/Broadsword path forces yaw-only orientation. A living
        // Force-grip handoff must instead sample the exact normal player pose on
        // its first frame, then Rapier freezes that skinning pose internally.
        let rapier_corpse = corpse_ragdoll_requested && self.ragdolls.active();
        let (axis, torso_angles) = if rapier_corpse {
            runtime.last_angle_time = current_time;
            (angles_to_axis([0.0, entity.angles[1], 0.0]), [0.0; 3])
        } else {
            // CG_Player calls CG_G2PlayerAngles (-> BG_G2PlayerAngles) before
            // CG_PlayerAnimation. The Motion bolt and the client's installed
            // legs/torso animation IDs queried here are therefore still last
            // frame's values: BG_G2ClientSpineAngles' entity-vs-client ID guard
            // depends on seeing that one-frame lag to detect an animation
            // change. Evaluate them ahead of `runtime.animation.update` below
            // to keep that ordering, rather than after it.
            let motion_index = Ghoul2Animator::bone_index(&model.gla, "Motion")
                .ok_or_else(|| format!("{} has no Ghoul2 Motion bone", info.model_qpath()))?;
            let pose_started = Instant::now();
            // OpenJK's CBoneCache::Eval is lazy: BG_G2PlayerAngles only needs the
            // pre-angle Motion bolt, so evaluate Motion + its ancestors here rather
            // than materializing all humanoid bones a first time.
            let motion_pose = runtime.animation.animator.evaluate_bone_openjk_root(
                &model.gla,
                current_time,
                motion_index,
            )?;
            record_motion_pose_eval(&mut self.perf, pose_started);
            let motion_bolt = multiply_3x4(
                &motion_pose,
                &model.gla.skeleton[motion_index].base_pose,
            );
            let motion_matrix = flatten_matrix3x4(&motion_bolt);

            let mut look_angles = entity.angles;
            if entity.state.field_i32("hasLookTarget").unwrap_or(0) != 0 {
                if let Some(target_origin) = look_target_origin {
                    look_angles = openjk_vectoangles([
                        target_origin[0] - entity.origin[0],
                        target_origin[1] - entity.origin[1],
                        target_origin[2] - entity.origin[2],
                    ]);
                    runtime.player_angles.look_time = current_time.saturating_add(1000);
                }
            }
            // Exact CG_Player behavior immediately before BG_G2PlayerAngles.
            look_angles[0] = 0.0;

            let frametime = current_time.saturating_sub(runtime.last_angle_time);
            let (ci_legs, ci_torso) = runtime.animation.client_animation_ids();
            let model_scale_int = entity.state.field_i32("iModelScale").unwrap_or(0);
            let model_scale = if model_scale_int != 0 {
                let scale = model_scale_int as f32 / 100.0;
                [scale, scale, scale]
            } else {
                [0.0; 3]
            };
            let angle_entity = player_angle_entity(entity);
            let angle_render_origin = ghoul2_render_origin(entity);
            let angle_result = bg_g2_player_angles(
                &angle_entity,
                current_time,
                angle_render_origin,
                entity.angles,
                frametime,
                model_scale,
                ci_legs,
                ci_torso,
                look_angles,
                Some(&motion_matrix),
                &mut runtime.player_angles,
            )?;
            runtime.last_angle_time = current_time;
            let torso_angles = [
                runtime.player_angles.torso_pitch_angle,
                runtime.player_angles.torso_yaw_angle,
                0.0,
            ];

            for command in angle_result.commands() {
                if command.flags & BONE_ANGLES_POSTMULT == 0 {
                    return Err(format!(
                        "OpenJK BG_G2PlayerAngles emitted unsupported bone-angle flags {:#x} for {}",
                        command.flags,
                        command.bone_name()
                    ));
                }
                runtime.animation.animator.set_bone_angles_postmult(
                    &model.gla,
                    command.bone_name(),
                    command.angles,
                    command.up,
                    command.right,
                    command.forward,
                )?;
            }
            (angle_result.axis(), torso_angles)
        };

        // CG_PlayerAnimation runs after CG_G2PlayerAngles: advance the legs/
        // torso lerp frames, client animation IDs, and Motion bone schedule
        // now that the angle calculation above has consumed last frame's
        // values.
        runtime.animation.update(
            &entity.state,
            &self.animations,
            &model.gla,
            info,
            &self.saber_scales,
            current_time,
            true,
        )?;

        let anim_debug = ViewerAnimDebug {
            legs_input: entity.state.field_i32("legsAnim").unwrap_or(0),
            torso_input: entity.state.field_i32("torsoAnim").unwrap_or(0),
            legs_anim: runtime.animation.legs.animation_number,
            legs_frame: runtime.animation.legs.frame,
            legs_old_frame: runtime.animation.legs.old_frame,
            legs_backlerp: runtime.animation.legs.backlerp,
            torso_anim: runtime.animation.torso.animation_number,
            torso_frame: runtime.animation.torso.frame,
            torso_old_frame: runtime.animation.torso.old_frame,
            torso_backlerp: runtime.animation.torso.backlerp,
            ..ViewerAnimDebug::default()
        };

        // CG_TriggerAnimSounds: the footsteps the legs bone just stepped onto.
        let mut footsteps = Vec::new();
        if self.footstep_stages.any()
            && entity.entity_type != ET_BODY
            && footsteps::npc_class_steps(entity.state.field_i32("NPC_class").unwrap_or(0))
        {
            if let Some(step) = runtime.animation.legs_frame_step() {
                let mut seed = self.footstep_rng;
                footsteps = footsteps::footsteps_between(
                    &self.anim_events.legs,
                    &self.animations,
                    step.anim,
                    step.old_frame,
                    step.frame,
                    || {
                        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                        ((seed >> 16) % 100) as i32
                    },
                );
                self.footstep_rng = seed;
            }
        }

        let cull_origin = ghoul2_render_origin(entity);
        let mut raster_visible = submit_geometry;

        // Keep the cgame-side player-angle/animation state advancing exactly
        // as it does for a visible entity, then mirror OpenJK's renderer-side
        // whole-Ghoul2 sphere rejection before the expensive final full pose.
        // Culled models therefore pay only the lazy Motion-bone chain above.
        let body_lod = if submit_geometry {
            if let Some(view) = view {
                if self.early_frustum_cull && entity.entity_type != ET_BODY {
                    self.perf.frustum_tests = self.perf.frustum_tests.saturating_add(1);
                    let radius = ghoul2_entity_radius(entity) * ghoul2_model_scale(entity);
                    if view.sphere_outside(cull_origin, radius) {
                        self.perf.frustum_culled = self.perf.frustum_culled.saturating_add(1);
                        if !self.rt_shadow_casters_enabled {
                            // Out of view is not out of earshot: the pose is not built,
                            // so the feet are taken to be under the entity.
                            if let (false, Some(world)) = (footsteps.is_empty(), self.collision_world.as_mut()) {
                                footsteps::push_footsteps(
                                    world,
                                    &mut self.footsteps,
                                    entity.number,
                                    runtime.player_angles.legs_yaw_angle,
                                    &footsteps,
                                    |_| footsteps::unposed_foot(entity.origin),
                                );
                            }
                            return Ok(Vec::new());
                        }
                        // Camera visibility is not shadow visibility: a caster
                        // just outside the viewport may still shadow visible BSP.
                        raster_visible = false;
                    }
                }
                let lod = ghoul2_lod_for_view(
                    view,
                    entity,
                    cull_origin,
                    model.glm.lods.len(),
                    self.lod_bias,
                    self.lod_scale,
                );
                let bucket = lod.min(self.perf.lod_counts.len() - 1);
                self.perf.lod_counts[bucket] = self.perf.lod_counts[bucket].saturating_add(1);
                lod
            } else {
                self.perf.lod_counts[0] = self.perf.lod_counts[0].saturating_add(1);
                0
            }
        } else {
            0
        };

        let pose_started = Instant::now();
        let mut pose = runtime
            .animation
            .evaluate_pose_openjk_root(&model.gla, current_time)?;
        record_pose_eval(&mut self.perf, pose_started);

        let (render_axis, render_origin) = ghoul2_render_transform(entity, axis);
        if ragdoll_requested {
            self.ragdolls.apply_to_pose(
                entity,
                &model.key,
                &model.gla,
                &mut pose,
                axis,
                render_origin,
                ghoul2_model_scale(entity),
                if corpse_ragdoll_requested {
                    RagdollMode::Corpse
                } else if force_grip_ragdoll_requested {
                    RagdollMode::ForceGrip
                } else {
                    RagdollMode::Impulse
                },
                impulse_ragdoll.map(|impulse| impulse.seed_velocity),
                current_time,
            )?;
        } else {
            self.ragdolls.apply_living_recovery(
                entity.number,
                &model.key,
                &mut pose,
                current_time,
            );
        }

        // jaPRO `r_ghoul2animsmooth`/`CBoneCache::SmoothLow`: filter the final
        // composed pose against last frame's filtered pose before it reaches
        // GPU skinning, bolt/attachment queries, and the draw pose below, so
        // all three stay consistent with each other. Ragdoll is its own
        // physically continuous pose; skip and reseed rather than blending an
        // animated-pose history across that discontinuity.
        if self.ghoul2_anim_smooth > 0.0 && self.ghoul2_anim_smooth < 1.0 && !ragdoll_requested {
            // A frustum-culled frame returns before this point without
            // touching `bone_smooth_time`, so a long-culled entity reappearing
            // is still caught here even though nothing reset the history above.
            const BONE_SMOOTH_MAX_GAP_MS: i32 = 250;
            if let Some(history) = runtime.bone_smooth_history.as_ref().filter(|history| {
                history.len() == pose.len()
                    && current_time.saturating_sub(runtime.bone_smooth_time) <= BONE_SMOOTH_MAX_GAP_MS
            }) {
                pose = smooth_ghoul2_pose(&model.gla, history, &pose, self.ghoul2_anim_smooth);
            }
            runtime.bone_smooth_history = Some(pose.clone());
            runtime.bone_smooth_time = current_time;
        } else {
            runtime.bone_smooth_history = None;
        }

        if entity.entity_type == ET_PLAYER && i32::from(entity.number) == self.viewer_client {
            self.viewer_foot_bolts = foot_bolts_timed(&mut self.perf, &model, &pose, ghoul2_model_scale(entity));
            let key_hash = {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                model.key.hash(&mut hasher);
                hasher.finish() as u32
            };
            let mut head_world = None;
            self.viewer_anim_debug = Some(ViewerAnimDebug {
                pose_time: current_time,
                origin: entity.origin,
                angles: entity.angles,
                feet: self.viewer_foot_bolts.unwrap_or_default(),
                bones: {
                    let scale = ghoul2_model_scale(entity);
                    let mut bone = |name| {
                        let matrix = model_bolt_matrix_timed(&mut self.perf, &model.glm, &model.gla, &pose, name)
                            .ok().flatten();
                        let point = matrix.map(|m| [m[0][3] * scale, m[1][3] * scale, m[2][3] * scale]);
                        if name == "*head_eyes" {
                            head_world = point.map(|point| transform_jka_model_point(point, render_axis, render_origin));
                        }
                        point.unwrap_or([0.0; 3])
                    };
                    [bone("*r_hand"), bone("*l_hand"), bone("*head_eyes"), bone("*chestg")]
                },
                head_world,
                render_origin,
                render_yaw: render_axis[0][1].atan2(render_axis[0][0]).to_degrees(),
                model_hash: key_hash,
                model_settled,
                ..anim_debug
            });
        }
        if let (false, Some(world)) = (footsteps.is_empty(), self.collision_world.as_mut()) {
            // CG_PlayerFootsteps: the foot bolt in the legs' own frame.
            let perf = &mut self.perf;
            footsteps::push_footsteps(
                world,
                &mut self.footsteps,
                entity.number,
                runtime.player_angles.legs_yaw_angle,
                &footsteps,
                |foot| {
                    let bolt = if foot.right_foot() { "*r_leg_foot" } else { "*l_leg_foot" };
                    model_bolt_origin_timed(perf, &model, &pose, render_axis, render_origin, bolt)
                        .ok()
                        .flatten()
                        .unwrap_or_else(|| footsteps::unposed_foot(entity.origin))
                },
            );
        }
        self.queue_force_fx(
            entity,
            &model,
            &pose,
            render_axis,
            render_origin,
            torso_angles,
            submit_geometry,
            current_time,
        )?;

        if !submit_geometry && !self.rt_shadow_casters_enabled {
            // OpenJK still runs CG_Player for the local player in first person
            // and marks the refEntity RF_THIRD_PERSON.  Keep all animation and
            // Ghoul2 state advancing, but suppress geometry submission here.
            self.report_player_status(entity.number, format!(
                "clientNum={} requested={}/{} resolved={} submitted=0 reason=first-person-body",
                info.client_num, info.model_name, info.skin_name, model.key,
            ));
            return Ok(Vec::new());
        }

        // `axis` is the `legs` matrix returned by the actual OpenJK
        // BG_G2PlayerAngles call, matching CG_Player's entity axis.
        // CG_Player: legs.shaderRGBA = cent->currentState.customRGBA (`char_color_*`).
        let look = self.look;
        let (body_rgba, body_rgb, body_custom) = match look.ghost {
            crate::japro_cg::Ghost::None => {
                let mut rgb = info.team_color_override.map_or_else(
                    || player_entity_rgb(entity),
                    |rgb| rgb.map(|channel| f32::from(channel) / 255.0),
                );
                if look.duel_bubble {
                    // Duelists seen from outside: shaderRGBA = 50 with RF_RGB_TINT.
                    rgb = [50.0 / 255.0; 3];
                } else if look.dim {
                    // Bystanders of your duel are drawn at a fifth of the light.
                    rgb = rgb.map(|channel| channel / 5.0);
                }
                ([1.0, 1.0, 1.0, entity_alpha], rgb, None)
            }
            ghost => {
                let mut rgba = crate::japro_cg::ghost_color(ghost);
                rgba[3] = entity_alpha;
                (rgba, [1.0; 3], Some(self.ghost_material(ghost)))
            }
        };
        let dismember_selection = source_dismember_surface_set(
            &model,
            self.dismembered.get(&entity.number),
        );
        if dismember_selection.is_some() {
            self.dismember_source_snaps.insert(
                entity.number,
                DismemberSourceSnap {
                    model: Arc::clone(&model),
                    pose: pose.clone(),
                    axis: render_axis,
                    origin: render_origin,
                    model_scale: ghoul2_model_scale(entity),
                    body_rgba,
                    body_rgb,
                    model1_weapon: detached_model1_weapon,
                    model1_primary_saber: detached_model1_primary_saber,
                    client_info: info.clone(),
                },
            );
        }

        // EternalJK FPLS mode 3 keeps the local Ghoul2 instance alive so its
        // attached saber hilt/blade presentation still works, but turns off the
        // player-body roots. Do not replace this with a hand/gun viewmodel:
        // EternalJK's saber FPLS path is the local Ghoul2 saber with the body
        // hidden, while CG_AddViewWeapon is the separate firearm viewmodel path.
        let fpls_surfaces;
        let body_surfaces = if first_person_saber {
            fpls_surfaces = model
                .surfaces
                .iter()
                .filter(|surface| !fpls_mode3_surface_hidden(&surface.surface_name))
                .cloned()
                .collect::<Vec<_>>();
            fpls_surfaces.as_slice()
        } else {
            model.surfaces.as_slice()
        };
        let mut draws = self.render_glm_surfaces_tinted_selected(
            entity.number,
            &model.key,
            &model.glm,
            &model.gla,
            body_surfaces,
            model.jiggle.as_deref(),
            &pose,
            body_lod,
            render_axis,
            render_origin,
            body_rgba,
            body_rgb,
            body_custom,
            true,
            dismember_selection.as_ref(),
        )?;

        // TaystJK CG_Player model index 3: when EF_JETPACK is present, bolt
        // models/weapons2/jetpack/model.glm to the player's *chestg bolt. The
        // same child pose owns torso_ljet/torso_rjet, which are also the
        // authoritative origins/directions for the stock Boba exhaust EFX.
        if e_flags & EF_JETPACK != 0 && e_flags & EF_DEAD == 0 {
            if let Err(error) = self.append_jetpack(
                &mut draws,
                entity,
                &model,
                &pose,
                render_axis,
                render_origin,
                current_time,
                entity_alpha,
            ) {
                self.report_saber_warning_once(entity.number, &format!("jetpack: {error}"));
            }
        }

        self.report_player_status(entity.number, format!(
            "clientNum={} requested={}/{} resolved={} bodySurfaces={} submitted={}",
            info.client_num, info.model_name, info.skin_name, model.key,
            draws.len(), usize::from(!draws.is_empty()),
        ));
        if self.gore_limit > 0 {
            let snaps = draws
                .iter()
                .filter_map(|surface| {
                    surface.ghoul2_gpu.as_ref().map(|skin| super::player_gore::GpuSnap {
                        mesh_key: Arc::clone(&skin.mesh_key),
                        vertices: Arc::clone(&skin.vertices),
                        indices: Arc::clone(&skin.indices),
                        bones: Arc::clone(&skin.bones),
                        jiggle_offsets: skin.jiggle_offsets,
                        axis: skin.axis,
                        origin: skin.origin,
                    })
                })
                .collect();
            self.last_gpu.insert(entity.number, snaps);
            self.append_gore(entity.number, current_time, &mut draws);
        }
        let shells = self.force_shells(entity, current_time);
        if !shells.is_empty() {
            let body = draws.clone();
            let seconds = self.stage_time_ms as f32 * 0.001;
            for (shader, rgba, force_alpha_blend) in shells {
                let (texture, mut alpha_mode) = self.custom_shader_material(shader);
                if force_alpha_blend {
                    // RF_FORCE_ENT_ALPHA: the team-power shell draws with
                    // standard alpha blending regardless of its own shader's
                    // (additive) blendFunc.
                    alpha_mode = DynamicModelAlphaMode::BlendUnlit;
                }
                let uv_xform = self.custom_shader_uv_xform(shader, seconds);
                let bulge_height = self.custom_shader_bulge_height(shader, seconds);
                let env_map = self.custom_shader_env_map(shader);
                let overlays = self.custom_shader_overlay_stages(shader, seconds, force_alpha_blend);
                for surface in &body {
                    let mut shell = Self::clone_surface_with_material_xform(
                        surface,
                        texture.clone(),
                        alpha_mode,
                        rgba,
                        Some(uv_xform),
                        Some(bulge_height),
                        Some(env_map),
                    );
                    // rgbGen identity/const customShader shells are flat-colored,
                    // scene-light-independent overlays (RF_RGB_TINT forces the
                    // color outright) and never their own shadow casters.
                    shell.lighting_origin = None;
                    shell.rt_rigid = None;
                    shell.rt_skinned_key = None;
                    draws.push(shell);
                    for (overlay_texture, overlay_mode, overlay_uv) in &overlays {
                        let mut overlay = Self::clone_surface_with_material_xform(
                            surface,
                            overlay_texture.clone(),
                            *overlay_mode,
                            rgba,
                            Some(*overlay_uv),
                            Some(bulge_height),
                            Some(env_map),
                        );
                        overlay.lighting_origin = None;
                        overlay.rt_rigid = None;
                        overlay.rt_skinned_key = None;
                        draws.push(overlay);
                    }
                }
            }
        }
        if submit_geometry {
            self.queue_cosmetics(entity, info, &model, &pose, render_axis, render_origin, look.ghost, entity_alpha);
        }
        if attached_sabers[0] || attached_sabers[1] {
            if let Err(error) = self.append_player_sabers(
                &mut draws, entity, info, &model, &pose, render_axis, render_origin, current_time, entity_alpha,
                attached_sabers, entity.entity_type != ET_BODY,
            ) {
                self.report_saber_warning_once(entity.number, &error);
            }
        }
        if let Some(weapon) = attached_weapon {
            if let Err(error) = self.append_held_weapon(
                &mut draws, entity, weapon, &model, &pose, render_axis, render_origin, current_time, entity_alpha,
            ) {
                self.report_saber_warning_once(entity.number, &format!("held weapon {weapon}: {error}"));
            }
        }
        if !raster_visible {
            for surface in &mut draws {
                surface.raster_visible = false;
            }
        }
        Ok(draws)
    }

    /// TaystJK/OpenJK CG_Player jetpack model-slot-3 presentation.
    ///
    /// The legacy renderer copies a preloaded Ghoul2 jetpack instance into
    /// model index 3, whose parent bolt is player bolt 2 (`*chestg`). This
    /// renderer has no mutable multi-model Ghoul2 instance, so the equivalent
    /// is a child GLM pose rooted directly at the chest bolt.
    #[allow(clippy::too_many_arguments)]
    fn append_jetpack(
        &mut self,
        draws: &mut Vec<DynamicModelSurface>,
        entity: &PresentedEntity,
        player_model: &PlayerModelAsset,
        player_pose: &[Matrix3x4],
        player_axis: [[f32; 3]; 3],
        player_origin: [f32; 3],
        current_time: i32,
        entity_alpha: f32,
    ) -> Result<(), String> {
        let Some(chest_bolt) = model_bolt_matrix_timed(
            &mut self.perf,
            &player_model.glm,
            &player_model.gla,
            player_pose,
            "*chestg",
        )? else {
            return Err(format!("{} has no Ghoul2 *chestg bolt", player_model.key));
        };

        let jetpack = self.load_static_glm_in_game(JETPACK_MODEL, None, JETPACK_MODEL)?;
        let pose_started = Instant::now();
        let jetpack_pose = Ghoul2Animator::new(&jetpack.gla)
            .evaluate_pose(&jetpack.gla, current_time, chest_bolt)?;
        record_pose_eval(&mut self.perf, pose_started);

        let mut jetpack_draws = self.render_glm_surfaces(
            entity.number,
            JETPACK_MODEL,
            &jetpack.glm,
            &jetpack.gla,
            &jetpack.surfaces,
            None,
            &jetpack_pose,
            0,
            player_axis,
            player_origin,
            [1.0, 1.0, 1.0, entity_alpha],
            None,
            true,
        )?;
        draws.append(&mut jetpack_draws);

        let e_flags = entity.state.field_i32("eFlags").unwrap_or(0);
        if e_flags & EF_JETPACK_ACTIVE == 0 {
            return Ok(());
        }

        if self.japro_cinfo2 & JAPRO_CINFO2_WTTRIBES != 0 {
            // TaystJK's WTTRIBES branch does not use the model nozzle bolts:
            // two vertical effects are placed around lerpOrigin using
            // AngleVectors(turAngles).
            // cent->turAngles is the legs angle output of BG_G2PlayerAngles;
            // player_axis is the same AnglesToAxis result (possibly uniformly
            // model-scaled), so normalize its basis back to directions.
            let forward = normalize_vec3(player_axis[0]);
            // AnglesToAxis stores model-left in axis[1]; AngleVectors returns
            // right, so negate it to match the C code.
            let model_left = normalize_vec3(player_axis[1]);
            let right = [-model_left[0], -model_left[1], -model_left[2]];
            let mut flame_pos = entity.origin;
            for i in 0..3 {
                flame_pos[i] -= 6.0 * forward[i];
            }
            flame_pos[2] += 22.0;
            for i in 0..3 {
                flame_pos[i] -= 6.0 * right[i];
            }
            self.fx_requests.push(PlayerFxRequest::EffectDir {
                name: "effects/tribes/jet.efx".to_owned(),
                origin: flame_pos,
                dir: [0.0, 0.0, 1.0],
            });
            for i in 0..3 {
                flame_pos[i] += 10.0 * right[i];
            }
            self.fx_requests.push(PlayerFxRequest::EffectDir {
                name: "effects/tribes/jet.efx".to_owned(),
                origin: flame_pos,
                dir: [0.0, 0.0, 1.0],
            });
            return Ok(());
        }

        // G2API_GetBoltMatrix applies the legacy Ghoul2 270-degree yaw fixup
        // before BG_GiveMeVectorFromMatrix sees the matrix. Our helper exposes
        // the lower/raw matrix, so reconstruct the same public X/Y axes here:
        // public +X = -raw column 1, public +Y = raw column 0.
        for (index, tag) in ["torso_ljet", "torso_rjet"].into_iter().enumerate() {
            let Some(matrix) = model_bolt_matrix_timed(
                &mut self.perf,
                &jetpack.glm,
                &jetpack.gla,
                &jetpack_pose,
                tag,
            )? else {
                continue;
            };
            let mut origin = transform_jka_model_point(
                [matrix[0][3], matrix[1][3], matrix[2][3]],
                player_axis,
                player_origin,
            );
            let positive_x = normalize_vec3(transform_jka_model_vector(
                [-matrix[0][1], -matrix[1][1], -matrix[2][1]],
                player_axis,
            ));
            let negative_y = normalize_vec3(transform_jka_model_vector(
                [-matrix[0][0], -matrix[1][0], -matrix[2][0]],
                player_axis,
            ));
            let (first_dir, final_dir) = if index == 0 {
                (negative_y, positive_x)
            } else {
                (positive_x, negative_y)
            };
            for i in 0..3 {
                origin[i] -= 9.5 * first_dir[i];
                origin[i] -= 13.5 * final_dir[i];
            }
            let request = || PlayerFxRequest::EffectDir {
                name: "effects/boba/jet.efx".to_owned(),
                origin,
                dir: final_dir,
            };
            self.fx_requests.push(request());
            // EF_JETPACK_FLAMING deliberately submits the same authored EFX
            // twice, matching TaystJK's FIXME-era behavior exactly.
            if e_flags & EF_JETPACK_FLAMING != 0 {
                self.fx_requests.push(request());
            }
        }
        Ok(())
    }

    /// Ghoul2 model index 1: the weapon's world GLM bolted to the player's
    /// right hand (G2API_SetBoltInfo(instance, 0, 0) -> bolt 0 = *r_hand).
    #[allow(clippy::too_many_arguments)]
    fn append_held_weapon(
        &mut self,
        draws: &mut Vec<DynamicModelSurface>,
        entity: &PresentedEntity,
        weapon: i32,
        player_model: &PlayerModelAsset,
        player_pose: &[Matrix3x4],
        player_axis: [[f32; 3]; 3],
        player_origin: [f32; 3],
        current_time: i32,
        entity_alpha: f32,
    ) -> Result<(), String> {
        let qpath = weapon_world_model(weapon).ok_or("weapon has no Ghoul2 world model")?;
        let Some(hand_bolt) = model_bolt_matrix_timed(&mut self.perf, &player_model.glm, &player_model.gla, player_pose, "*r_hand")? else {
            return Err(format!("{} has no Ghoul2 *r_hand bolt", player_model.key));
        };
        let weapon_model = self.load_static_glm_in_game(&qpath, None, &qpath)?;
        let pose_started = Instant::now();
        let pose = Ghoul2Animator::new(&weapon_model.gla).evaluate_pose(&weapon_model.gla, current_time, hand_bolt)?;
        record_pose_eval(&mut self.perf, pose_started);
        let mut weapon_draws = self.render_glm_surfaces(
            entity.number,
            &qpath,
            &weapon_model.glm,
            &weapon_model.gla,
            &weapon_model.surfaces,
            None,
            &pose,
            0,
            player_axis,
            player_origin,
            [1.0, 1.0, 1.0, entity_alpha],
            None,
            true,
        )?;
        draws.append(&mut weapon_draws);
        Ok(())
    }

    fn load_model(
        &mut self,
        info: &crate::cgame::ClientInfo,
    ) -> Result<Arc<PlayerModelAsset>, String> {
        match self.try_load_model(info) {
            Ok(model) => Ok(model),
            Err(primary_error) => {
                let fallback = info.missing_model_fallback();
                self.try_load_model(&fallback).map_err(|fallback_error| format!(
                    "{primary_error}; fallback {}/{} failed: {fallback_error}",
                    fallback.model_name, fallback.skin_name,
                ))
            }
        }
    }

    fn report_player_status(&mut self, entity_num: u16, status: String) {
        if self.player_diagnostics.get(&entity_num) != Some(&status) {
            devprintln!(2, "PLAYER PRESENTATION: entity={entity_num} {status}");
            self.player_diagnostics.insert(entity_num, status);
        }
    }

    fn try_load_model(
        &mut self,
        info: &crate::cgame::ClientInfo,
    ) -> Result<Arc<PlayerModelAsset>, String> {
        let key = player_model_key(info);
        match self.lookup_model(&key) {
            ModelLookup::Ready(model) => return Ok(model),
            ModelLookup::Failed(error) => return Err(error),
            // Synchronous callers (previews, `cg_asyncAssets 0`) need the
            // answer now; an in-flight async copy just becomes redundant.
            ModelLookup::Pending | ModelLookup::Missing => {}
        }

        let result = self.load_model_uncached(info, key.clone());
        match result {
            Ok(model) => {
                let model = Arc::new(model);
                self.models.insert(key, Arc::clone(&model));
                Ok(model)
            }
            Err(error) => {
                println!("PLAYER MODEL REGISTRATION FAILED: {key}: {error}");
                self.failed_models.insert(key, error.clone());
                Err(error)
            }
        }
    }

    fn load_model_uncached(
        &mut self,
        info: &crate::cgame::ClientInfo,
        key: String,
    ) -> Result<PlayerModelAsset, String> {
        build_player_model(&mut SyncModelSource { presenter: self }, info, key)
    }

    fn lookup_model(&self, key: &str) -> ModelLookup {
        if let Some(model) = self.models.get(key) {
            return ModelLookup::Ready(Arc::clone(model));
        }
        if let Some(error) = self.failed_models.get(key) {
            return ModelLookup::Failed(error.clone());
        }
        match self.async_models.state(key) {
            Some(AssetState::Ready(model)) => ModelLookup::Ready(Arc::clone(model)),
            Some(AssetState::Failed(error)) => ModelLookup::Failed(error.clone()),
            Some(AssetState::Pending) => ModelLookup::Pending,
            None => ModelLookup::Missing,
        }
    }

    /// Queue `info`'s model on the asset workers unless it is already known.
    /// Returns immediately; cache hits and pending requests cost one lookup.
    fn request_player_model(&mut self, info: &crate::cgame::ClientInfo, key: &str) -> ModelLookup {
        let lookup = self.lookup_model(key);
        if !matches!(lookup, ModelLookup::Missing) {
            if matches!(lookup, ModelLookup::Pending) {
                self.async_models.note_duplicate();
            }
            return lookup;
        }
        let source = Arc::clone(&self.asset_source);
        let shared = Arc::clone(&self.load_shared);
        let job_info = info.clone();
        let job_key = key.to_owned();
        let requested = self
            .async_models
            .request(key, AssetPriority::High, move || {
                asset_jobs::with_worker_vfs(&source, |vfs| {
                    build_player_model(
                        &mut WorkerModelSource { vfs, shared: &shared },
                        &job_info,
                        job_key,
                    )
                })?
            });
        match requested {
            Requested::Ready(model) => ModelLookup::Ready(model),
            Requested::Pending => ModelLookup::Pending,
            Requested::Failed(error) => ModelLookup::Failed(error),
            Requested::Rejected => ModelLookup::Missing,
        }
    }

    fn async_loading_enabled(&self) -> bool {
        self.async_loading && asset_jobs::async_available()
    }

    #[allow(dead_code)]
    pub fn set_async_loading(&mut self, enabled: bool) {
        self.async_loading = enabled;
    }

    /// Per-frame bounded drain of finished asset jobs. A no-op (one integer
    /// compare) whenever nothing is in flight.
    fn poll_asset_completions(&mut self) {
        if self.async_models.pending_count() > 0 {
            self.async_models.drain(16);
        }
        if self.async_static_models.pending_count() > 0 {
            self.async_static_models.drain(16);
        }
    }

    /// Warm the fallback bodies OpenJK falls back to on registration failure so
    /// a brand-new remote player has something to show while their real model
    /// loads. Runs on the workers; nothing blocks.
    fn prime_default_player_models(&mut self) {
        if !self.async_loading_enabled() {
            return;
        }
        for name in ["kyle", "jan"] {
            let info = crate::cgame::ClientInfo::solo_model(name);
            let key = player_model_key(&info);
            self.request_player_model(&info, &key);
        }
    }

    /// Model to present for `info` without ever blocking on IO.
    ///
    /// The desired key and the cache are distinct: a completion only fills its
    /// own key, so a stale finish can never overwrite a newer selection, and a
    /// model finished for one player is reused by every other player.
    fn resolve_player_model_async(&mut self, info: &crate::cgame::ClientInfo) -> ModelResolution {
        let key = player_model_key(info);
        let fallback = info.missing_model_fallback();
        match self.request_player_model(info, &key) {
            ModelLookup::Ready(model) => return ModelResolution::Ready(model),
            ModelLookup::Failed(primary_error) => {
                // CG_LoadClientInfo failure policy: same as the synchronous path.
                let fallback_key = player_model_key(&fallback);
                return match self.request_player_model(&fallback, &fallback_key) {
                    ModelLookup::Ready(model) => ModelResolution::Ready(model),
                    ModelLookup::Failed(fallback_error) => ModelResolution::Failed(format!(
                        "{primary_error}; fallback {}/{} failed: {fallback_error}",
                        fallback.model_name, fallback.skin_name,
                    )),
                    ModelLookup::Pending | ModelLookup::Missing => ModelResolution::Loading,
                };
            }
            ModelLookup::Pending | ModelLookup::Missing => {}
        }
        // Not ready yet: show the OpenJK fallback body if it is already
        // resident (the plain default bodies are primed at creation). Only peek;
        // never queue extra loads on behalf of a model that is merely pending.
        let plain = crate::cgame::ClientInfo::solo_model(if info.female { "jan" } else { "kyle" });
        for candidate in [&fallback, &plain] {
            let candidate_key = player_model_key(candidate);
            if candidate_key == key {
                continue;
            }
            if let ModelLookup::Ready(model) = self.lookup_model(&candidate_key) {
                return ModelResolution::Provisional(model);
            }
        }
        ModelResolution::Loading
    }

    /// Re-scan the active VFS namespace and discard remembered registration
    /// failures. Successful model caches remain hot; this is the targeted
    /// `fs_refresh` boundary for content that appeared on disk after CGame was
    /// created, not a destructive CGame restart.
    pub fn retry_failed_assets(&mut self) -> Result<usize, String> {
        self.assets
            .refresh()
            .map_err(|error| format!("PLAYER ASSET REFRESH ERROR: {error}"))?;

        // A newly mounted package may contribute shader definitions needed by
        // the model that previously failed. Rebuild the lightweight definition
        // table while preserving already-decoded model/texture caches.
        let mut shader_warnings = Vec::new();
        let (shaders, diagnostics) =
            materials::shader_library(&mut self.assets, &mut shader_warnings, self.pbr)?;
        self.shaders = Arc::new(shaders);
        for warning in shader_warnings {
            rverbose!(1, "PLAYER MATERIAL REFRESH WARNING: {warning}");
        }
        println!(
            "PLAYER ASSET REFRESH: shaderDefs={} mtrDefs={}",
            diagnostics.shader_definitions, diagnostics.mtr_definitions
        );

        let retried = self.async_models.retry_failed() + self.async_static_models.retry_failed();
        self.failed_models.clear();
        self.failed_saber_models.clear();
        self.reported_saber_warnings.clear();
        self.reported_saber_diagnostics.clear();
        self.reported_vehicle_fallbacks.clear();
        self.asset_source = AssetSource::from_search_path(&self.assets);
        // Worker-side GLA/texture caches remember failures too. A new load-shared
        // object makes future jobs resolve against the refreshed shader table.
        self.load_shared = Arc::new(ModelLoadShared::new(Arc::clone(&self.shaders)));
        self.blob_shadow_texture_resolved = false;
        self.blob_shadow_asset_warned = false;
        if retried > 0 {
            devprintln!(1, "[ASSET] retrying {retried} previously failed player asset(s)");
        }
        Ok(retried)
    }

    fn register_client_saber_assets(
        &mut self,
        info: &crate::cgame::ClientInfo,
        entity_num: u16,
    ) {
        // OpenJK WP_SetSaber removes saber slot 1 for "none"/"remove" (and for
        // two-handed combinations) and leaves an empty model. Those slots are
        // skipped rather than fed through the missing-definition fallback,
        // which would incorrectly create the stock Reborn hilt.
        let equipped = self.equipped_sabers(info);
        for (saber_name, definition) in
            [info.saber_name.as_str(), info.saber2_name.as_str()]
                .into_iter()
                .zip(equipped)
        {
            let Some(definition) = definition else { continue };
            if saber_name.is_empty() {
                continue;
            }
            if let Err(error) = self.load_saber_model_in_game(&definition) {
                self.report_saber_warning_once(entity_num, &error);
            }
        }
    }

    /// The `clientInfo_t::saber[]` pair `WP_SetSaber` produces for this client.
    fn equipped_sabers(&self, info: &crate::cgame::ClientInfo) -> [Option<SaberDefinition>; 2] {
        self.saber_definitions
            .equip([&info.saber_name, &info.saber2_name], info.client_num < MAX_CLIENTS)
    }

    /// The equipped sabers as Pmove sees them through BG_MySaber.
    pub fn saber_movement_loadout(
        &self,
        saber_names: [&str; 2],
        player_client: bool,
    ) -> [jka_movement::SaberMovementInfo; 2] {
        crate::local_server::saber_movement_loadout(
            &self.saber_definitions,
            saber_names,
            player_client,
        )
    }

    fn report_saber_warning_once(&mut self, entity_num: u16, error: &str) {
        if is_asset_pending(error) {
            return;
        }
        let key = error.to_ascii_lowercase();
        if self.reported_saber_warnings.insert(key) {
            println!("PLAYER SABER WARNING ent={entity_num}: {error}");
        }
    }

    fn report_saber_diagnostic_once(&mut self, key: String, message: String) {
        if crate::logging::developer_enabled(2) && self.reported_saber_diagnostics.insert(key) {
            println!("{message}");
        }
    }

    fn presented_saber_blade_length(
        &mut self,
        client_num: usize,
        saber_num: usize,
        blade_num: usize,
        authored_length: f32,
        desired_length: f32,
        current_time: i32,
    ) -> f32 {
        let key = SaberBladeLengthKey {
            client_num,
            saber_num: saber_num as u8,
            blade_num: blade_num as u8,
        };
        let state = self
            .saber_blade_lengths
            .entry(key)
            .or_insert_with(|| SaberBladeLengthState::new(authored_length, desired_length, current_time));
        state.set_desired_and_update(desired_length, authored_length, current_time)
    }

    fn append_player_sabers(
        &mut self,
        draws: &mut Vec<DynamicModelSurface>,
        entity: &PresentedEntity,
        info: &crate::cgame::ClientInfo,
        player_model: &PlayerModelAsset,
        player_pose: &[Matrix3x4],
        player_axis: [[f32; 3]; 3],
        player_origin: [f32; 3],
        current_time: i32,
        entity_alpha: f32,
        attached_sabers: [bool; 2],
        process_blades: bool,
    ) -> Result<(), String> {
        let dead = entity.state.field_i32("eFlags").unwrap_or(0) & EF_DEAD != 0;
        let weapon = entity.state.field_i32("weapon").unwrap_or(0);
        let holstered = entity.state.field_i32("saberHolstered").unwrap_or(0);
        let in_flight = entity.state.field_i32("saberInFlight").unwrap_or(0) != 0;
        let saber_move = entity.state.field_i32("saberMove").unwrap_or(0);
        let torso_anim = entity.state.field_i32("torsoAnim").unwrap_or(0);
        let mut sabers = Vec::with_capacity(2);
        // These booleans are the Rust equivalent of actual Ghoul2 model
        // indices 1/2. Hilt visibility follows those slots, not holster/death
        // guesses from currentState. A slot must also exist in the client's
        // WP_SetSaber result.
        let [primary, secondary] = self.equipped_sabers(info);
        if let (true, Some(definition)) = (attached_sabers[0], primary) {
            sabers.push((0usize, info.saber_name.as_str(), info.saber_color, "*r_hand", definition));
        }
        if let (true, Some(definition)) = (attached_sabers[1], secondary) {
            sabers.push((1usize, info.saber2_name.as_str(), info.saber2_color, "*l_hand", definition));
        }

        for (saber_num, saber_name, saber_color, hand_name, definition) in sabers {
            let Some(hand_bolt) = model_bolt_matrix_timed(
                &mut self.perf,
                &player_model.glm,
                &player_model.gla,
                player_pose,
                hand_name,
            )? else {
                return Err(format!("{} has no Ghoul2 {} bolt", player_model.key, hand_name));
            };
            let hilt = self.load_saber_model_in_game(&definition)?;
            let mut animator = Ghoul2Animator::new(&hilt.gla);
            let pose_started = Instant::now();
            let hilt_pose = animator.evaluate_pose(&hilt.gla, current_time, hand_bolt)?;
            record_pose_eval(&mut self.perf, pose_started);
            if i32::from(entity.number) == self.viewer_client {
                if let Some(debug) = self.viewer_anim_debug.as_mut() {
                    debug.hilt_world[saber_num] = Some(transform_jka_model_point(
                        [hand_bolt[0][3], hand_bolt[1][3], hand_bolt[2][3]], player_axis, player_origin,
                    ));
                }
            }
            let mut hilt_draws = self.render_glm_surfaces(
                entity.number,
                &definition.model,
                &hilt.glm,
                &hilt.gla,
                &hilt.surfaces,
                None,
                &hilt_pose,
                0,
                player_axis,
                player_origin,
                [1.0, 1.0, 1.0, entity_alpha],
                None,
                false,
            )?;
            if i32::from(entity.number) == self.viewer_client {
                if let Some(debug) = self.viewer_anim_debug.as_mut() {
                    debug.hilt_submitted[saber_num] = hilt_draws.iter().any(|draw| draw.raster_visible);
                }
            }
            // Two saber slots may legitimately use the exact same hilt model.
            // Keep their RT BLAS identities distinct while material-shell clones
            // of one physical hilt continue sharing the same caster identity.
            if self.rt_shadow_casters_enabled {
                for surface in &mut hilt_draws {
                    if let Some(key) = surface.rt_skinned_key.take() {
                        surface.rt_skinned_key = Some(Arc::<str>::from(format!(
                            "{}#held-saber{}",
                            key.as_ref(),
                            saber_num,
                        )));
                    }
                }
            }
            draws.append(&mut hilt_draws);
            // ET_BODY goes through CG_General in OpenJK. The copied hilt model
            // remains, but CG_Player/CG_AddSaberBlade never runs for the body.
            if !process_blades {
                continue;
            }

            // Process every authored blade, including blades that are turning
            // off. OpenJK still advances those lengths and calls the saber-blade
            // path with dontDraw once they reach zero; our zero-length request
            // is what clears WeaponFx trail/contact history.
            for blade_index in 0..definition.num_blades {
                let tag_name = format!("*blade{}", blade_index + 1);
                let (blade_bolt, resolved_tag) = if let Some(matrix) = model_bolt_matrix_timed(
                    &mut self.perf,
                    &hilt.glm,
                    &hilt.gla,
                    &hilt_pose,
                    &tag_name,
                )? {
                    (matrix, tag_name.as_str())
                } else if blade_index == 0 {
                    if let Some(matrix) = model_bolt_matrix_timed(
                        &mut self.perf,
                        &hilt.glm,
                        &hilt.gla,
                        &hilt_pose,
                        "*flash",
                    )? {
                        (matrix, "*flash")
                    } else {
                        self.report_saber_diagnostic_once(
                            format!(
                                "held:{}:{}:{}:missing-bolt",
                                entity.number, saber_num, blade_index
                            ),
                            format!(
                                "SABER PRESENT FAILED ent={} saber={} name={} stage=bladeBolt tried={},*flash",
                                entity.number, saber_num, saber_name, tag_name
                            ),
                        );
                        continue;
                    }
                } else {
                    self.report_saber_diagnostic_once(
                        format!(
                            "held:{}:{}:{}:missing-bolt",
                            entity.number, saber_num, blade_index
                        ),
                        format!(
                            "SABER PRESENT FAILED ent={} saber={} name={} stage=bladeBolt tried={}",
                            entity.number, saber_num, saber_name, tag_name
                        ),
                    );
                    continue;
                };
                let origin_model = [blade_bolt[0][3], blade_bolt[1][3], blade_bolt[2][3]];
                // Public G2API_GetBoltMatrix applies its historical 90-degree
                // column fix.  CG_Player then requests NEGATIVE_Y, which reduces
                // to -column0 of the low/internal bolt matrix used here.
                let dir_model = normalize_vec3([
                    -blade_bolt[0][0],
                    -blade_bolt[1][0],
                    -blade_bolt[2][0],
                ]);
                let origin_world = transform_jka_model_point(origin_model, player_axis, player_origin);
                let dir_world = normalize_vec3(transform_jka_model_vector(dir_model, player_axis));
                let blade = definition.blade(blade_index);
                // stillDoSaber in OpenJK:
                // - EF_DEAD + WP_SABER => both sabers desired 0, gradual
                // - saberHolstered==1 => primary blade0 on, extra/second off
                // - saberHolstered>=2 => both desired 0, gradual
                // - non-saber weapon => lengths are immediately zeroed
                let desired_length = if weapon != WP_SABER || dead || holstered >= 2 {
                    0.0
                } else if holstered == 1 && (saber_num > 0 || blade_index > 0) {
                    0.0
                } else {
                    -1.0
                };
                let presented_length = if weapon == WP_SABER {
                    self.presented_saber_blade_length(
                        info.client_num,
                        saber_num,
                        blade_index,
                        blade.length,
                        desired_length,
                        current_time,
                    )
                } else {
                    let key = SaberBladeLengthKey {
                        client_num: info.client_num,
                        saber_num: saber_num as u8,
                        blade_num: blade_index as u8,
                    };
                    self.saber_blade_lengths.insert(
                        key,
                        SaberBladeLengthState::new(blade.length, 0.0, current_time),
                    );
                    0.0
                };
                let client_color = if saber_num == 0 && self.saber_staff_multi_color && blade_index >= 1 {
                    info.saber2_color
                } else {
                    saber_color
                };
                let saber_color = blade_color(info, client_color, &blade, Some(self.saber_team_colors));
                if i32::from(entity.number) == self.viewer_client && blade_index < 8 {
                    if let Some(debug) = self.viewer_anim_debug.as_mut() {
                        debug.blade_world[saber_num][blade_index] = Some(origin_world);
                        debug.blade_core_world[saber_num][blade_index] = Some(std::array::from_fn(|i| origin_world[i] - dir_world[i]));
                        debug.blade_length[saber_num][blade_index] = presented_length;
                        debug.blade_submitted[saber_num][blade_index] = process_blades && presented_length > 0.0 && entity_alpha >= 8.0 / 255.0;
                    }
                }
                let secondary_style = definition.blade_style2_start > 0
                    && blade_index >= definition.blade_style2_start;
                let trail_style = if secondary_style {
                    definition.trail_style2
                } else {
                    definition.trail_style
                };
                let no_wall_marks = if secondary_style {
                    definition.no_wall_marks2
                } else {
                    definition.no_wall_marks
                };
                if let Some(request) = saber_blade_fx_request(
                    origin_world,
                    dir_world,
                    presented_length,
                    blade.length,
                    blade.radius,
                    saber_color,
                    entity_alpha,
                    entity.number,
                    saber_num as u8,
                    blade_index as u8,
                    saber_move,
                    torso_anim,
                    in_flight,
                    trail_style,
                    definition.num_blades as u8,
                    definition.no_dlight,
                    no_wall_marks,
                ) {
                    self.fx_requests.push(request);
                }
                self.report_saber_diagnostic_once(
                    format!("held:{}:{}:{}:submitted", entity.number, saber_num, blade_index),
                    format!(
                        "SABER PRESENT ent={} saber={} name={} handBolt={} blade={} tag={} origin=({:.1},{:.1},{:.1}) dir=({:.3},{:.3},{:.3}) length={:.1} radius={:.1} color={} submitted=1",
                        entity.number,
                        saber_num,
                        saber_name,
                        hand_name,
                        blade_index,
                        resolved_tag,
                        origin_world[0], origin_world[1], origin_world[2],
                        dir_world[0], dir_world[1], dir_world[2],
                        presented_length,
                        blade.radius,
                        saber_color,
                    ),
                );
            }
        }
        Ok(())
    }

    /// OpenJK's thrown saber is not an ordinary generic-model submission.
    /// `CG_Player` follows `saberEntityNum`, manually renders that Ghoul2 hilt,
    /// and evaluates the primary saber's blade bolts from the thrown entity.
    pub fn present_thrown_saber_for_player(
        &mut self,
        owner: &PresentedEntity,
        info: &crate::cgame::ClientInfo,
        entities: &[PresentedEntity],
        game: &ClientGameState,
        current_time: i32,
        entity_alpha: f32,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let mut draws = Vec::new();
        // CG_Player only runs the thrown-saber render inside its
        // `weapon == WP_SABER && saberHolstered < 2` branch.
        if owner.state.field_i32("weapon").unwrap_or(0) != WP_SABER
            || owner.state.field_i32("saberHolstered").unwrap_or(0) >= 2
            || owner.state.field_i32("saberInFlight").unwrap_or(0) == 0
            || owner.state.field_i32("saberEntityNum").unwrap_or(0) == 0
        {
            self.thrown_sabers.remove(&owner.number);
            return Ok(draws);
        }

        let saber_entity_num = owner.state.field_i32("saberEntityNum").unwrap_or(-1);
        let Ok(saber_entity_num_u16) = u16::try_from(saber_entity_num) else {
            self.thrown_sabers.remove(&owner.number);
            self.report_saber_diagnostic_once(
                format!("throw:{}:bad-entity", owner.number),
                format!(
                    "SABER THROW FAILED owner={} stage=saberEntityNum value={saber_entity_num}",
                    owner.number
                ),
            );
            return Ok(draws);
        };
        let Some(saber_entity) = entities
            .iter()
            .find(|entity| entity.number == saber_entity_num_u16)
        else {
            self.thrown_sabers.remove(&owner.number);
            self.report_saber_diagnostic_once(
                format!("throw:{}:{}:missing-entity", owner.number, saber_entity_num),
                format!(
                    "SABER THROW FAILED owner={} saberEnt={} stage=entityLookup",
                    owner.number, saber_entity_num
                ),
            );
            return Ok(draws);
        };

        let [primary, _] = self.equipped_sabers(info);
        let Some(mut definition) = primary else {
            self.thrown_sabers.remove(&owner.number);
            return Ok(draws);
        };
        let model_index = saber_entity.state.field_i32("modelindex").unwrap_or(0);
        let network_model = game.model_qpath(model_index);
        if let Some(qpath) = network_model
            .as_ref()
            .filter(|qpath| qpath.to_ascii_lowercase().ends_with(".glm"))
        {
            // The thrown entity's network-visible model is authoritative, just
            // like OpenJK's CS_MODELS lookup before G2API_InitGhoul2Model.
            definition.model.clone_from(qpath);
        }
        let hilt = self.load_saber_model_in_game(&definition)?;
        let mut animator = Ghoul2Animator::new(&hilt.gla);
        let pose_started = Instant::now();
        let hilt_pose = animator.evaluate_pose_openjk_root(&hilt.gla, current_time)?;
        record_pose_eval(&mut self.perf, pose_started);
        let snapshot_num = game.current_snapshot().map_or(0, |snapshot| snapshot.message_num);
        let throw_state = self.thrown_sabers.entry(owner.number).or_insert_with(|| {
            SaberThrowState::new(owner, saber_entity, current_time, snapshot_num)
        });
        let hilt_angles = throw_state.angles(
            owner, saber_entity, current_time, snapshot_num, definition.return_damage,
        )?;
        let saber_axis = angles_to_axis(hilt_angles);
        let blade_axis = angles_to_axis(blade_angles(hilt_angles));
        let outbound = saber_entity.state.field_i32("saberInFlight").unwrap_or(0) != 0;
        self.report_saber_diagnostic_once(
            format!("throw-axis:{}:{}:{outbound}", owner.number, saber_entity.number),
            format!(
                "SABER THROW AXIS owner={} saberEnt={} outbound={} angles={:?} aposType={} yawVelocity={} returnDamage={}",
                owner.number, saber_entity.number, outbound, hilt_angles,
                saber_entity.state.field_i32("apos.trType").unwrap_or(0),
                saber_entity.state.field_f32("apos.trDelta[1]").unwrap_or(0.0),
                definition.return_damage,
            ),
        );

        let mut hilt_draws = self.render_glm_surfaces(
            saber_entity.number,
            &definition.model,
            &hilt.glm,
            &hilt.gla,
            &hilt.surfaces,
            None,
            &hilt_pose,
            0,
            saber_axis,
            saber_entity.origin,
            [1.0, 1.0, 1.0, entity_alpha],
            None,
            false,
        )?;
        if i32::from(owner.number) == self.viewer_client {
            if let Some(debug) = self.viewer_anim_debug.as_mut() {
                debug.hilt_world[0] = Some(saber_entity.origin);
                debug.hilt_submitted[0] = hilt_draws.iter().any(|draw| draw.raster_visible);
            }
        }
        draws.append(&mut hilt_draws);

        let holstered = owner.state.field_i32("saberHolstered").unwrap_or(0);
        for blade_index in 0..definition.num_blades {
            if holstered == 1 && blade_index > 0 {
                // A staff thrown in single-blade mode: the extra blades get a
                // desired length of 0 and CG_AddSaberBlade(dontDraw). Keep
                // their persistent length state advancing without drawing.
                let authored = definition.blade(blade_index).length;
                self.presented_saber_blade_length(
                    info.client_num, 0, blade_index, authored, 0.0, current_time,
                );
                continue;
            }
            let tag_name = format!("*blade{}", blade_index + 1);
            let (blade_bolt, resolved_tag) = if let Some(matrix) = model_bolt_matrix_timed(
                &mut self.perf,
                &hilt.glm,
                &hilt.gla,
                &hilt_pose,
                &tag_name,
            )? {
                (matrix, tag_name.as_str())
            } else if blade_index == 0 {
                if let Some(matrix) = model_bolt_matrix_timed(
                    &mut self.perf,
                    &hilt.glm,
                    &hilt.gla,
                    &hilt_pose,
                    "*flash",
                )? {
                    (matrix, "*flash")
                } else {
                    self.report_saber_diagnostic_once(
                        format!("throw:{}:{}:missing-bolt", owner.number, blade_index),
                        format!(
                            "SABER THROW FAILED owner={} saberEnt={} model={} stage=bladeBolt tried={},*flash",
                            owner.number, saber_entity.number, definition.model, tag_name
                        ),
                    );
                    continue;
                }
            } else {
                self.report_saber_diagnostic_once(
                    format!("throw:{}:{}:missing-bolt", owner.number, blade_index),
                    format!(
                        "SABER THROW FAILED owner={} saberEnt={} model={} stage=bladeBolt tried={}",
                        owner.number, saber_entity.number, definition.model, tag_name
                    ),
                );
                continue;
            };

            let origin_model = [blade_bolt[0][3], blade_bolt[1][3], blade_bolt[2][3]];
            let dir_model = normalize_vec3([
                -blade_bolt[0][0],
                -blade_bolt[1][0],
                -blade_bolt[2][0],
            ]);
            let origin_world =
                transform_jka_model_point(origin_model, blade_axis, saber_entity.origin);
            let dir_world = normalize_vec3(transform_jka_model_vector(dir_model, blade_axis));
            let blade = definition.blade(blade_index);
            let presented_length = self.presented_saber_blade_length(
                info.client_num, 0, blade_index, blade.length, -1.0, current_time,
            );
            let client_color = if self.saber_staff_multi_color && blade_index >= 1 {
                info.saber2_color
            } else {
                info.saber_color
            };
            let saber_color = blade_color(info, client_color, &blade, Some(self.saber_team_colors));
            if i32::from(owner.number) == self.viewer_client && blade_index < 8 {
                if let Some(debug) = self.viewer_anim_debug.as_mut() {
                    debug.blade_world[0][blade_index] = Some(origin_world);
                    debug.blade_core_world[0][blade_index] = Some(std::array::from_fn(|i| origin_world[i] - dir_world[i]));
                    debug.blade_length[0][blade_index] = presented_length;
                    debug.blade_submitted[0][blade_index] = presented_length > 0.0 && entity_alpha >= 8.0 / 255.0;
                }
            }
            let secondary_style = definition.blade_style2_start > 0
                && blade_index >= definition.blade_style2_start;
            let trail_style = if secondary_style {
                definition.trail_style2
            } else {
                definition.trail_style
            };
            let no_wall_marks = if secondary_style {
                definition.no_wall_marks2
            } else {
                definition.no_wall_marks
            };
            if let Some(request) = saber_blade_fx_request(
                origin_world,
                dir_world,
                presented_length,
                blade.length,
                blade.radius,
                saber_color,
                entity_alpha,
                owner.number,
                0,
                blade_index as u8,
                owner.state.field_i32("saberMove").unwrap_or(0),
                owner.state.field_i32("torsoAnim").unwrap_or(0),
                true,
                trail_style,
                definition.num_blades as u8,
                definition.no_dlight,
                no_wall_marks,
            ) {
                self.fx_requests.push(request);
            }
            self.report_saber_diagnostic_once(
                format!("throw:{}:{}:submitted", owner.number, blade_index),
                format!(
                    "SABER THROW owner={} saberEnt={} name={} model={} modelindex={} blade={} tag={} origin=({:.1},{:.1},{:.1}) dir=({:.3},{:.3},{:.3}) length={:.1} radius={:.1} color={} submitted=1",
                    owner.number,
                    saber_entity.number,
                    info.saber_name,
                    definition.model,
                    model_index,
                    blade_index,
                    resolved_tag,
                    origin_world[0], origin_world[1], origin_world[2],
                    dir_world[0], dir_world[1], dir_world[2],
                    presented_length,
                    blade.radius,
                    saber_color,
                ),
            );
        }

        Ok(draws)
    }

    fn vehicle_fallback(
        &mut self,
        entity_num: u16,
        requested: &str,
        resolved_glm: Option<&str>,
        resolved_skin: Option<&str>,
        primary_error: &str,
    ) -> Result<Arc<SaberModelAsset>, String> {
        const SWOOP: &str = "models/players/swoop/model.glm";
        const SWOOP_SKIN: &str = "models/players/swoop/model_default.skin";
        let resolved = resolved_glm.unwrap_or("<unresolved vehicle definition>");
        let warning_key = format!(
            "{}|{}|{}",
            requested,
            resolved,
            resolved_skin.unwrap_or("")
        )
        .to_ascii_lowercase();
        if self.reported_vehicle_fallbacks.insert(warning_key) {
            rverbose!(
                1,
                "VEHICLE MODEL FALLBACK: entity={} requested={} resolved={} skin={} reason={}; using {}",
                entity_num,
                requested,
                resolved,
                resolved_skin.unwrap_or("<default>"),
                primary_error,
                SWOOP,
            );
        }
        self.load_static_glm_in_game(SWOOP, Some(SWOOP_SKIN), SWOOP)
            .map_err(|fallback_error| {
                format!(
                    "vehicle {requested} ({resolved}) failed ({primary_error}); fallback {SWOOP} failed: {fallback_error}"
                )
            })
    }

    fn load_saber_model(&mut self, definition: &SaberDefinition) -> Result<Arc<SaberModelAsset>, String> {
        self.load_static_glm(&definition.model, definition.custom_skin.as_deref(), &definition.name)
    }

    fn load_saber_model_in_game(&mut self, definition: &SaberDefinition) -> Result<Arc<SaberModelAsset>, String> {
        self.load_static_glm_in_game(&definition.model, definition.custom_skin.as_deref(), &definition.name)
    }

    /// Gameplay/presentation registration of a Ghoul2 hilt, weapon or vehicle
    /// model. Cache hits are one lookup. A miss queues the load on the asset
    /// workers and returns an `ASSET_PENDING` error, which callers treat as
    /// "nothing to draw yet"; explicit/preview paths keep `load_static_glm`.
    fn load_static_glm_in_game(
        &mut self,
        model_qpath: &str,
        custom_skin: Option<&str>,
        label: &str,
    ) -> Result<Arc<SaberModelAsset>, String> {
        if !self.async_loading_enabled() {
            return self.load_static_glm(model_qpath, custom_skin, label);
        }
        let key = static_model_key(model_qpath, custom_skin, false);
        if let Some(model) = self.saber_models.get(&key) {
            return Ok(Arc::clone(model));
        }
        if self.failed_saber_models.contains(&key) {
            return Err(format!("Ghoul2 model {model_qpath} failed registration earlier"));
        }
        match self.async_static_models.state(&key) {
            Some(AssetState::Ready(model)) => return Ok(Arc::clone(model)),
            Some(AssetState::Failed(error)) => return Err(error.clone()),
            Some(AssetState::Pending) => {
                self.async_static_models.note_duplicate();
                return Err(format!("Ghoul2 model {model_qpath}: {ASSET_PENDING}"));
            }
            None => {}
        }
        let source = Arc::clone(&self.asset_source);
        let shared = Arc::clone(&self.load_shared);
        let job_qpath = model_qpath.to_owned();
        let job_skin = custom_skin.map(str::to_owned);
        let job_label = label.to_owned();
        match self
            .async_static_models
            .request(&key, AssetPriority::High, move || {
                asset_jobs::with_worker_vfs(&source, |vfs| {
                    build_static_glm(
                        &mut WorkerModelSource { vfs, shared: &shared },
                        &job_qpath,
                        job_skin.as_deref(),
                        &job_label,
                        false,
                    )
                })?
            }) {
            Requested::Ready(model) => Ok(model),
            Requested::Failed(error) => Err(error),
            Requested::Pending | Requested::Rejected => {
                Err(format!("Ghoul2 model {model_qpath}: {ASSET_PENDING}"))
            }
        }
    }

    /// A Ghoul2 model drawn in its own default pose (saber hilts, weapon
    /// item world models): G2API_InitGhoul2Model with an optional skin.
    fn load_static_glm(
        &mut self,
        model_qpath: &str,
        custom_skin: Option<&str>,
        label: &str,
    ) -> Result<Arc<SaberModelAsset>, String> {
        self.load_static_glm_with_options(model_qpath, custom_skin, label, false)
    }

    fn load_static_glm_with_options(
        &mut self,
        model_qpath: &str,
        custom_skin: Option<&str>,
        label: &str,
        preview_fallback: bool,
    ) -> Result<Arc<SaberModelAsset>, String> {
        let key = static_model_key(model_qpath, custom_skin, preview_fallback);
        if let Some(model) = self.saber_models.get(&key) {
            return Ok(Arc::clone(model));
        }
        if self.failed_saber_models.contains(&key) {
            return Err(format!("Ghoul2 model {model_qpath} failed registration earlier"));
        }
        let result = self.load_static_glm_uncached(model_qpath, custom_skin, label, preview_fallback);
        match result {
            Ok(model) => {
                let model = Arc::new(model);
                self.saber_models.insert(key, Arc::clone(&model));
                Ok(model)
            }
            Err(error) => {
                self.failed_saber_models.insert(key);
                Err(error)
            }
        }
    }

    fn load_static_glm_uncached(
        &mut self,
        model_qpath: &str,
        custom_skin: Option<&str>,
        label: &str,
        preview_fallback: bool,
    ) -> Result<SaberModelAsset, String> {
        build_static_glm(
            &mut SyncModelSource { presenter: self },
            model_qpath,
            custom_skin,
            label,
            preview_fallback,
        )
    }

    /// jaPRO CG_PlayerSprites, called from CG_Player once the client is valid and
    /// not EF_NODRAW: at most one icon floats 48 units over the head - connection
    /// trouble first, else a voice command, else the talk balloon (never on NPCs).
    /// `visible` is false for the viewer's own body in first person, where
    /// CG_PlayerFloatSprite marks the sprite RF_THIRD_PERSON (mirrors only).
    pub fn queue_player_sprites(&mut self, entity: &PresentedEntity, game: &ClientGameState, current_time: i32, visible: bool) {
        if entity.entity_type != ET_PLAYER || !visible {
            return;
        }
        let state = &entity.state;
        let e_flags = state.field_i32("eFlags").unwrap_or(0);
        if e_flags & EF_NODRAW != 0
            || state.field_i32("eFlags2").unwrap_or(0) & EF2_SHIP_DEATH != 0
            || entity_is_mind_tricked(entity, self.viewer_client)
        {
            return;
        }
        let voice = game.voice_chat_until(i32::from(entity.number)) > current_time;
        let Some(shader) = head_sprite_shader(e_flags, voice) else { return };
        let mut origin = entity.origin;
        origin[2] += 48.0;
        self.fx_requests.push(PlayerFxRequest::HeadSprite { origin, shader });
    }

    /// CG_Player's force-power effect block: drain/lightning effects at the
    /// hand bolts, push/pull (PW_DISINT_4) and grip puffs, and the
    /// EF_BODYPUSH full-body blur.
    #[allow(clippy::too_many_arguments)]
    fn queue_force_fx(
        &mut self,
        entity: &PresentedEntity,
        model: &PlayerModelAsset,
        pose: &[Matrix3x4],
        axis: [[f32; 3]; 3],
        render_origin: [f32; 3],
        torso_angles: [f32; 3],
        third_person: bool,
        current_time: i32,
    ) -> Result<(), String> {
        let state = &entity.state;
        let pass = state.field_i32("activeForcePass").unwrap_or(0);
        let vehicle = state.field_i32("NPC_class").unwrap_or(0) == crate::cgame::CLASS_VEHICLE;
        if pass != 0 && !vehicle {
            let fx_axis = angles_to_axis(torso_angles);
            let request = if pass > FORCE_LEVEL_3 {
                let level = pass - FORCE_LEVEL_3;
                let wide = level > FORCE_LEVEL_2;
                // cg_drainFX: 0 off, 1 stock mp/drain(wide).efx, 2 (default)
                // jaPRO mp/drain(wide)_japro.efx - cgs.effects.forceDrain(Wide)[JaPRO].
                match self.japro.drain_fx {
                    0 => None,
                    1 => Some(if wide { "mp/drainwide" } else { "mp/drain" }),
                    _ => Some(if wide { "mp/drainwide_japro" } else { "mp/drain_japro" }),
                }
                .map(|name| (name, "*l_hand"))
            } else {
                // BOTH_FORCE_2HANDEDLIGHTNING_HOLD alternates hands (Q_irand).
                // Force lightning itself is not governed by cg_drainFX.
                let two_handed = animation_number("BOTH_FORCE_2HANDEDLIGHTNING_HOLD")
                    .is_some_and(|anim| state.field_i32("torsoAnim").unwrap_or(-1) & !0x800 == anim);
                let hand = if two_handed && (current_time / 50) & 1 == 1 { "*r_hand" } else { "*l_hand" };
                Some((if pass > FORCE_LEVEL_2 { "force/lightningwide" } else { "force/lightning" }, hand))
            };
            if let Some((name, hand)) = request {
                if let Some(origin) = model_bolt_origin_timed(
                    &mut self.perf, model, pose, axis, render_origin, hand,
                )? {
                    self.fx_requests.push(PlayerFxRequest::Effect { name, origin, axis: fx_axis });
                }
            }
        }
        if state.field_i32("eFlags").unwrap_or(0) & EF_BODYPUSH != 0 {
            for bone in PUSH_BONE_NAMES {
                if let Some(index) = Ghoul2Animator::bone_index(&model.gla, bone) {
                    let m = multiply_3x4(&pose[index], &model.gla.skeleton[index].base_pose);
                    let origin = transform_jka_model_point([m[0][3], m[1][3], m[2][3]], axis, render_origin);
                    self.fx_requests.push(PlayerFxRequest::PushPuffs { origin });
                }
            }
        }
        if state.field_i32("powerups").unwrap_or(0) & (1 << PW_DISINT_4) != 0 {
            if let Some(origin) = model_bolt_origin_timed(
                &mut self.perf, model, pose, axis, render_origin, "*l_hand",
            )? {
                let gripping = state.field_i32("forcePowersActive").unwrap_or(0) & (1 << FP_GRIP) != 0;
                if gripping && third_person {
                    self.fx_requests.push(PlayerFxRequest::GripPuffs { origin });
                    self.fx_requests.push(PlayerFxRequest::GripPuffs { origin });
                } else if !gripping {
                    // cg_renderToTextureFX 0 path. The default refraction
                    // half-shield (which grows for PW_PULL, shrinks for push)
                    // needs a screen-copy distortion pass (not ported yet).
                    self.fx_requests.push(PlayerFxRequest::PushPuffs { origin });
                }
            }
        }
        Ok(())
    }

    /// CG_Player's extra full-body refEntities with a customShader.
    /// `(shader, rgba, force_alpha_blend)`. `force_alpha_blend` mirrors
    /// CG_Player's `RF_FORCE_ENT_ALPHA` on the team-power shell: that shell is
    /// the only one here the stock engine draws with standard alpha blending
    /// (and `rgbGen`/`blendFunc` overridden to the entity colour) instead of
    /// its own shader's `blendFunc`. Every other shell keeps drawing with its
    /// authored additive blend.
    fn force_shells(&self, entity: &PresentedEntity, current_time: i32) -> Vec<(&'static str, [f32; 4], bool)> {
        let state = &entity.state;
        let active = state.field_i32("forcePowersActive").unwrap_or(0);
        let mut shells = Vec::new();
        if active & (1 << FP_RAGE) != 0 {
            // rand() & 1 every rendered frame (not a quantized tick) between the
            // two electric shaders; folding in the entity number decorrelates
            // simultaneous ragers the way independent rand() calls would.
            let hash = (current_time as u32)
                .wrapping_mul(0x9E37_79B1)
                .wrapping_add(u32::from(entity.number).wrapping_mul(0x85EB_CA6B));
            let shader = if (hash >> 24) & 1 == 0 { "gfx/misc/electric" } else { "gfx/misc/fullbodyelectric2" };
            shells.push((shader, [1.0, 0.0, 0.0, 1.0], false));
        }
        let number = i32::from(entity.number);
        if self.look.duel_bubble && number != self.viewer_client && entity.entity_type == ET_PLAYER {
            // Duelists seen from outside the duel.
            shells.push(("gfx/misc/sightbubble", [100.0 / 255.0, 100.0 / 255.0, 1.0, 1.0], false));
        }
        if let Some(gray) = self.duel_shell_gray {
            // jaPRO cg_stylePlayer "duel shell": you and your opponent glow.
            let partner = self.viewer_style.duel_index;
            if entity.entity_type == ET_PLAYER && (number == partner || number == self.viewer_client) {
                shells.push(("powerups/forceshell", [gray, gray, gray, 1.0], false));
            }
        }
        if active & (1 << FP_PROTECT) != 0 {
            shells.push(("gfx/misc/forceprotect", [0.0, 128.0 / 255.0, 0.0, 254.0 / 255.0], false));
        }
        if let Some(&(until, kind)) = self.team_power.get(&entity.number) {
            let remaining = until - current_time;
            if remaining > 0 {
                if kind == 3 {
                    // Absorb: blue playerShieldDamage shell.
                    shells.push(("gfx/misc/personalshield", [0.0, 0.0, 1.0, 254.0 / 255.0], false));
                } else {
                    let rgb = match kind {
                        1 => [0.0, 1.0, 0.0], // heal
                        0 => [0.0, 0.0, 1.0], // regen
                        _ => [1.0, 0.0, 0.0], // drain
                    };
                    // shaderRGBA[3] = (teamPowerEffectTime - cg.time) / 8, a byte.
                    let alpha = ((remaining / 8).min(255)) as f32 / 255.0;
                    shells.push(("powerups/ysalimarishell", [rgb[0], rgb[1], rgb[2], alpha], true));
                }
            }
        }
        if state.field_i32("isJediMaster").unwrap_or(0) != 0 && number != self.viewer_client {
            shells.push(("powerups/forceshell", [100.0 / 255.0, 100.0 / 255.0, 1.0, 1.0], false));
        }
        shells
    }

    /// Material of a jaPRO ghost shader (`raceShader`/`duelShader`). Stock
    /// servers do not ship them, so fall back to the force shell they wrap.
    fn ghost_material(&mut self, ghost: crate::japro_cg::Ghost) -> (Option<Arc<TextureData>>, DynamicModelAlphaMode) {
        let name = crate::japro_cg::ghost_shader(ghost).unwrap_or("powerups/forceshell");
        if jka_assets::shader::find_shader(&self.shaders, name).is_some() {
            self.custom_shader_material(name)
        } else {
            self.custom_shader_material("powerups/forceshell")
        }
    }

    /// jaPRO's private-duel shell brightness: `255 - distance / 4` to your
    /// opponent (distance clamped to 1..1024). `None` outside your own duel or
    /// without `JAPRO_STYLE_SHELL`.
    fn duel_shell_brightness(
        &self,
        game: &ClientGameState,
        origins: &HashMap<u16, [f32; 3]>,
        preserve_entity: Option<u16>,
    ) -> Option<f32> {
        if !self.viewer_style.dueling || !self.japro.style_bit(crate::japro_cg::style::SHELL) {
            return None;
        }
        let partner = u16::try_from(self.viewer_style.duel_index).ok()?;
        let ps = &game.current_snapshot()?.player_state;
        let viewer = [ps.field_f32("origin[0]")?, ps.field_f32("origin[1]")?, ps.field_f32("origin[2]")?];
        let other = origins.get(&partner).copied().filter(|_| preserve_entity != Some(partner))?;
        let distance = ((other[0] - viewer[0]).powi(2) + (other[1] - viewer[1]).powi(2) + (other[2] - viewer[2]).powi(2))
            .sqrt()
            .clamp(1.0, 1024.0);
        Some(((255.0 - distance / 4.0).max(1.0)) / 255.0)
    }

    /// `CG_DrawCosmeticOnPlayer`: hats bolted to `*head_top`, capes and held
    /// items to `*back`, from the jaPRO `c5` cosmetics mask. The MD3s themselves
    /// are submitted by the entity presenter (see [`CosmeticDraw`]).
    #[allow(clippy::too_many_arguments)]
    fn queue_cosmetics(
        &mut self,
        entity: &PresentedEntity,
        info: &crate::cgame::ClientInfo,
        model: &PlayerModelAsset,
        pose: &[Matrix3x4],
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        ghost: crate::japro_cg::Ghost,
        entity_alpha: f32,
    ) {
        use crate::japro_cg;
        let state = &entity.state;
        // Only jaPRO servers relay everyone's `c5`. A local game has no server, so
        // the viewer's own `cp_cosmetics` is drawn directly. Stock servers draw none.
        let own = i32::from(entity.number) == self.viewer_client;
        let japro_server = self.viewer_style.japro;
        if !(japro_server || own)
            || entity.entity_type != ET_PLAYER
            || self.japro.style_bit(japro_cg::style::HIDE_COSMETICS)
            || state.field_i32("eFlags").unwrap_or(0) & EF_DEAD != 0
            || entity_is_mind_tricked(entity, self.viewer_client)
        {
            return;
        }
        let mut mask = if japro_server { info.cosmetics } else { self.local_cosmetics };
        if japro_server && mask == 0 && self.japro.style_bit(japro_cg::style::SEASONAL_COSMETICS) {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs() as i64);
            let (month, day) = japro_cg::month_day_utc(now);
            mask = japro_cg::seasonal_cosmetic(month, day).unwrap_or(0);
        }
        if japro_server
            && own
            && info.cosmetics != self.sent_cosmetics
            && self.cosmetic_mismatch_logged != Some((self.sent_cosmetics, info.cosmetics))
        {
            self.cosmetic_mismatch_logged = Some((self.sent_cosmetics, info.cosmetics));
            println!(
                "COSMETICS: sent cp_cosmetics={} but the server relays c5={};                  the server removes cosmetics this account has not unlocked",
                self.sent_cosmetics as i32, info.cosmetics as i32,
            );
        }
        if mask == 0 {
            return;
        }
        let draws = cosmetic_draws_for_mask(
            &mut self.perf,
            model,
            pose,
            axis,
            origin,
            mask,
            entity.number,
            [1.0, 1.0, 1.0, entity_alpha],
            japro_cg::ghost_shader(ghost),
        );
        self.cosmetic_draws.extend(draws);
    }

    /// OpenJK MP CG_PlayerShadow for cg_shadows 1. The original submits a
    /// temporary mark via CG_ImpactMark/R_MarkFragments, which clips the mark
    /// onto the *rendered* BSP surfaces. We keep the trace, stock markShadow
    /// image, radius, yaw, fade and eligibility rules and submit one plane-aligned
    /// request per blob at the collision hit; the renderer does the fragment
    /// clipping against the rendered floor and batches every blob into one mesh.
    fn queue_blob_shadow(&mut self, entity: &PresentedEntity) {
        if !self.blob_shadows_enabled {
            return;
        }
        let state = &entity.state;
        if state.field_i32("eFlags").unwrap_or(0) & EF_DEAD != 0
            || state.field_i32("powerups").unwrap_or(0) & (1 << PW_CLOAKED) != 0
            || entity_is_mind_tricked(entity, self.viewer_client)
        {
            return;
        }
        let npc_class = state.field_i32("NPC_class").unwrap_or(0);
        if state.field_i32("m_iVehicleNum").unwrap_or(0) != 0 && npc_class != crate::cgame::CLASS_VEHICLE {
            return;
        }

        let Some(world) = self.collision_world.as_mut() else {
            return;
        };
        let mut end = entity.origin;
        end[2] -= BLOB_SHADOW_DISTANCE;
        let trace = world.trace(TraceQuery {
            start: entity.origin,
            mins: BLOB_SHADOW_MINS,
            maxs: BLOB_SHADOW_MAXS,
            end,
            pass_entity: 0,
            mask: MASK_PLAYERSOLID,
        });
        if trace.fraction == 1.0 || trace.start_solid != 0 || trace.all_solid != 0 {
            return;
        }

        // CG_PlayerShadow uses cent->pe.legs.yawAngle. At this point in
        // CG_Player that is the persistent playerEntity angle from the previous
        // presentation update; a newly-seen entity falls back to current yaw.
        let yaw = self
            .entities
            .get(&entity.number)
            .map(|runtime| runtime.player_angles.legs_yaw_angle)
            .unwrap_or(entity.angles[1]);
        let radius = if npc_class == CLASS_REMOTE || npc_class == CLASS_SEEKER {
            BLOB_SHADOW_DROID_RADIUS
        } else {
            BLOB_SHADOW_RADIUS
        };
        let shade = (1.0 - trace.fraction).clamp(0.0, 1.0);
        self.push_blob_shadow_request(entity.number, trace.end, trace.normal, yaw, radius, shade);
    }

    fn push_blob_shadow_request(
        &mut self,
        entity_number: u16,
        origin: [f32; 3],
        normal: [f32; 3],
        yaw_degrees: f32,
        radius: f32,
        shade: f32,
    ) {
        let normal = Vec3::from_array(normal).normalize_or_zero();
        if normal.length_squared() < 1.0e-8 {
            return;
        }
        // Build an orthonormal tangent frame, then rotate it by the exact
        // legs-yaw angle CG_ImpactMark receives. markShadow is essentially
        // radial, but keeping the rotation preserves OpenJK semantics.
        let reference = if normal.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
        let tangent0 = reference.cross(normal).normalize_or_zero();
        if tangent0.length_squared() < 1.0e-8 {
            return;
        }
        let bitangent0 = normal.cross(tangent0).normalize_or_zero();
        let (sin_yaw, cos_yaw) = yaw_degrees.to_radians().sin_cos();
        let tangent = (tangent0 * cos_yaw + bitangent0 * sin_yaw) * radius;
        let bitangent = (-tangent0 * sin_yaw + bitangent0 * cos_yaw) * radius;
        // The renderer recovers the projection axis as cross(left, up), so the
        // frame must stay right-handed with the hit normal: tangent0 x
        // bitangent0 = normal, and a rotation about it preserves that. The
        // render-space swizzle is a proper rotation, so it does too.
        self.blob_shadow_instances.push(FxGpuSpriteInstance {
            origin: {
                let c = scene::render_position(origin);
                [c[0], c[1], c[2], shade]
            },
            left: {
                let t = scene::render_position(tangent.to_array());
                [t[0], t[1], t[2], f32::from(entity_number)]
            },
            up: {
                let b = scene::render_position(bitangent.to_array());
                [b[0], b[1], b[2], 0.0]
            },
            color: [0.0; 4],
        });
    }

    /// A refEntity customShader: its first stage's texture with FX blending
    /// semantics (these shaders are unlit, vertex/entity colored).
    fn custom_shader_material(&mut self, shader_name: &str) -> (Option<Arc<TextureData>>, DynamicModelAlphaMode) {
        let key = shader_name.replace('\\', "/").to_ascii_lowercase();
        let stage = jka_assets::shader::find_shader(&self.shaders, shader_name)
            .and_then(Shader::primary)
            .cloned();
        let (image, clamp, blend) = match &stage {
            Some(stage) => (stage.image.clone(), stage.clamp, stage.blend.clone()),
            None => (key.clone(), false, String::new()),
        };
        let texture = if image.eq_ignore_ascii_case("$whiteimage") {
            None
        } else {
            self.textures.load(&mut self.assets, &image, clamp).map(|index| {
                if !self.texture_arcs.contains_key(&index) {
                    self.texture_arcs.insert(index, Arc::new(self.textures.images[index].clone()));
                }
                Arc::clone(&self.texture_arcs[&index])
            })
        };
        (texture, crate::fx::draw::FxBlend::from_blend_func(&blend).custom_shader_alpha_mode())
    }

    /// A refEntity customShader's `tcMod scroll`/`scale` chain (e.g. the Rage
    /// shells' `gfx/misc/electric`/`fullbodyelectric2`, which scroll+tile the
    /// lightning texture instead of mapping it 1:1 onto the body's own UVs).
    fn custom_shader_uv_xform(&self, shader_name: &str, seconds: f32) -> [f32; 4] {
        let tc_mods = jka_assets::shader::find_shader(&self.shaders, shader_name)
            .and_then(Shader::primary)
            .map(|stage| stage.tc_mods.clone())
            .unwrap_or_default();
        stage_uv_xform(&tc_mods, seconds)
    }

    /// A refEntity customShader's `deformVertexes bulge` offset (e.g. the Rage
    /// shells, which JKA's `RB_CalcBulgeVertexes` special-cases into a
    /// constant push outward along the vertex normal rather than the stock id
    /// Tech 3 time/UV-varying sine wave; see `Bulge::is_static`). A
    /// time-varying `deformVertexes wave` (e.g. the team-power shell
    /// `powerups/ysalimarishell`'s `wave 100 sin 0 1 0 1` pulse) is
    /// approximated the same way: one oscillating value pushed uniformly
    /// along every vertex's normal, dropping the real formula's per-vertex
    /// `(x+y+z)/div` phase spread (there is no per-draw slot to carry it
    /// through to the GPU skin, only this single scalar).
    fn custom_shader_bulge_height(&self, shader_name: &str, seconds: f32) -> f32 {
        let Some(shader) = jka_assets::shader::find_shader(&self.shaders, shader_name) else { return 0.0 };
        if let Some(bulge) = shader.bulge.filter(Bulge::is_static) {
            return bulge.height;
        }
        shader.vertex_wave.map_or(0.0, |vertex_wave| wave_value(vertex_wave.wave, seconds))
    }

    /// A refEntity customShader's `tcGen environment` (e.g. Absorb's
    /// `gfx/misc/personalshield` chrome stage): reflection-vector UVs instead
    /// of the base mesh's own, same as `RB_CalcEnvironmentTexCoords`.
    fn custom_shader_env_map(&self, shader_name: &str) -> bool {
        jka_assets::shader::find_shader(&self.shaders, shader_name)
            .and_then(Shader::primary)
            .is_some_and(|stage| stage.tc_gen == TcGen::Environment)
    }

    /// Additive stages of a refEntity customShader beyond its primary (e.g.
    /// the team-power shell's second `glow` + `tcMod turb` pass, which gives
    /// it its swirling sheen on top of the base tint). `custom_shader_material`
    /// only reads stage 0, mirroring OpenJK's common single-xstage shells;
    /// this covers the few that layer a second additive pass on top.
    /// Stages needing per-vertex features this path doesn't carry (`tcGen
    /// environment`/vector, alpha test) are skipped. `force_alpha_blend`
    /// carries the base layer's `RF_FORCE_ENT_ALPHA` override so a follow-on
    /// stage blends the same way instead of fighting it with its own additive
    /// `blendFunc`.
    fn custom_shader_overlay_stages(
        &mut self,
        shader_name: &str,
        seconds: f32,
        force_alpha_blend: bool,
    ) -> Vec<(Option<Arc<TextureData>>, DynamicModelAlphaMode, [f32; 4])> {
        use crate::fx::draw::FxBlend;
        let Some(shader) = jka_assets::shader::find_shader(&self.shaders, shader_name) else { return Vec::new() };
        let Some(primary) = shader.primary() else { return Vec::new() };
        let overlays: Vec<Stage> = shader
            .stages
            .iter()
            .filter(|stage| !std::ptr::eq(*stage, primary))
            .filter(|stage| {
                !stage.image.is_empty()
                    && !stage.image.starts_with('$')
                    && !stage.entity_rgb
                    && stage.alpha_test.trim().is_empty()
                    && matches!(stage.tc_gen, TcGen::Base)
                    && matches!(FxBlend::from_blend_func(&stage.blend), FxBlend::Add | FxBlend::AddAlpha)
            })
            .cloned()
            .collect();
        overlays
            .into_iter()
            .map(|stage| {
                let texture = if stage.image.eq_ignore_ascii_case("$whiteimage") {
                    None
                } else {
                    self.textures.load(&mut self.assets, &stage.image, stage.clamp).map(|index| {
                        if !self.texture_arcs.contains_key(&index) {
                            self.texture_arcs.insert(index, Arc::new(self.textures.images[index].clone()));
                        }
                        Arc::clone(&self.texture_arcs[&index])
                    })
                };
                let alpha_mode = if force_alpha_blend {
                    DynamicModelAlphaMode::BlendUnlit
                } else {
                    FxBlend::from_blend_func(&stage.blend).custom_shader_alpha_mode()
                };
                let uv_xform = stage_uv_xform(&stage.tc_mods, seconds);
                (texture, alpha_mode, uv_xform)
            })
            .collect()
    }

    /// Submit a Ghoul2 model in its default pose as one refEntity: `axis` may
    /// carry a non-uniform scale (CG_Item's 1.5x weapons), `rgba` is the
    /// RF_RGB_TINT/RF_FORCE_ENT_ALPHA shaderRGBA and `custom_shader` replaces
    /// every surface's material like refEntity.customShader.
    pub fn present_static_glm(
        &mut self,
        entity_num: u16,
        model_qpath: &str,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        rgba: [f32; 4],
        custom_shader: Option<&str>,
        current_time: i32,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let mut model = self.load_static_glm(model_qpath, None, model_qpath)?;
        // TaystJK/OpenJK CG_General initializes the Ghoul2 model first, then
        // checks G2API_SkinlessModel and, when needed, applies the sibling
        // `model_default.skin`. Our no-skin registration yields no drawable
        // surfaces for that same class of GLM, so perform the identical
        // fallback before submitting the entity. A missing default skin is
        // non-fatal in OpenJK (R_RegisterSkin returns handle 0), so retain the
        // original registration if this optional lookup fails.
        if model.surfaces.is_empty() {
            if let Some(default_skin) = sibling_default_skin_qpath(model_qpath) {
                if let Ok(skinned) = self.load_static_glm(model_qpath, Some(&default_skin), model_qpath) {
                    model = skinned;
                }
            }
        }
        let pose_started = Instant::now();
        let pose = Ghoul2Animator::new(&model.gla).evaluate_pose_openjk_root(&model.gla, current_time)?;
        record_pose_eval(&mut self.perf, pose_started);
        let custom = custom_shader.map(|shader| self.custom_shader_material(shader));
        let draws = self.render_glm_surfaces(
            entity_num,
            model_qpath,
            &model.glm,
            &model.gla,
            &model.surfaces,
            None,
            &pose,
            0,
            axis,
            origin,
            rgba,
            custom,
            true,
        )?;
        Ok(draws)
    }

    /// Profile preview path for a real JKA player selection. Unlike the generic
    /// Asset Viewer, this resolves the exact model[/skin] through ClientInfo, so
    /// multipart skins and `*off` surface mappings behave exactly like CG_Player.
    // Asset-preview entry point that is not wired into the viewer yet.
    #[allow(dead_code)]
    pub fn present_static_player_preview(
        &mut self,
        info: &crate::cgame::ClientInfo,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        current_time: i32,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let model = self.load_model(info)?;
        let pose_started = Instant::now();
        let pose = Ghoul2Animator::new(&model.gla)
            .evaluate_pose_openjk_root(&model.gla, current_time)?;
        record_pose_eval(&mut self.perf, pose_started);
        self.render_glm_surfaces(
            0,
            &info.model_qpath(),
            &model.glm,
            &model.gla,
            &model.surfaces,
            model.jiggle.as_deref(),
            &pose,
            0,
            axis,
            origin,
            [1.0; 4],
            None,
            true,
        )
    }

    /// Modern Profile viewport: stock humanoid stand animation plus the actual
    /// selected saber hilts bolted to the same hand tags used by CG_Player.
    /// This is deliberately a thin presentation path over the normal assets;
    /// model/skin/saber semantics remain OpenJK-compatible userinfo values.
    pub fn present_profile_player_preview(
        &mut self,
        info: &crate::cgame::ClientInfo,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        current_time: i32,
        animation_name: &str,
        with_sabers: bool,
        skin_tint: [f32; 3],
        cosmetics: u32,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let model = self.load_model(info)?;
        let animation_number = animation_index(animation_name)
            .ok_or_else(|| format!("missing animation table entry {animation_name}"))? as i32;
        let animation = *self
            .animations
            .get(animation_number)
            .ok_or_else(|| format!("missing humanoid animation {animation_name}"))?;
        if animation.frame_lerp == 0 {
            return Err(format!("humanoid animation {animation_name} has zero frameLerp"));
        }
        let anim_speed = 50.0 / f32::from(animation.frame_lerp);
        let (first_frame, last_frame) = if anim_speed < 0.0 {
            (
                i32::from(animation.first_frame) + i32::from(animation.num_frames),
                i32::from(animation.first_frame),
            )
        } else {
            (
                i32::from(animation.first_frame),
                i32::from(animation.first_frame) + i32::from(animation.num_frames),
            )
        };
        let flags = if animation.loop_frames != -1 {
            BONE_ANIM_OVERRIDE_LOOP
        } else {
            BONE_ANIM_OVERRIDE_FREEZE
        };

        let mut animator = Ghoul2Animator::new(&model.gla);
        // CG_Player uses model_root for legs and lower_lumbar for torso. For a
        // full-body menu stand both use the same authored animation; Motion is
        // updated too, matching the same-animation branch in CG_PlayerAnimation.
        for bone in ["model_root", "lower_lumbar", "Motion"] {
            if Ghoul2Animator::bone_index(&model.gla, bone).is_some() {
                animator.set_bone_anim(
                    &model.gla,
                    bone,
                    first_frame,
                    last_frame,
                    flags,
                    anim_speed,
                    0,
                    None,
                    0,
                )?;
            }
        }
        let pose_started = Instant::now();
        let pose = animator.evaluate_pose_openjk_root(&model.gla, current_time.max(0))?;
        record_pose_eval(&mut self.perf, pose_started);
        let mut draws = self.render_glm_surfaces_tinted(
            0,
            &info.model_qpath(),
            &model.glm,
            &model.gla,
            &model.surfaces,
            model.jiggle.as_deref(),
            &pose,
            0,
            axis,
            origin,
            [1.0; 4],
            skin_tint,
            None,
            true,
        )?;

        if with_sabers {
            for (saber_num, saber_name, hand_name) in [
                (0usize, info.saber_name.as_str(), "*r_hand"),
                (1usize, info.saber2_name.as_str(), "*l_hand"),
            ] {
                if saber_name.is_empty() || saber_name_is_removed(saber_name) {
                    continue;
                }
                let definition = self.saber_definitions.definition_or_default(saber_name);
                let Some(hand_bolt) = model_bolt_matrix_timed(
                    &mut self.perf,
                    &model.glm,
                    &model.gla,
                    &pose,
                    hand_name,
                )? else {
                    continue;
                };
                let hilt = self.load_saber_model(&definition)?;
                let mut hilt_animator = Ghoul2Animator::new(&hilt.gla);
                let pose_started = Instant::now();
                let hilt_pose = hilt_animator.evaluate_pose(&hilt.gla, current_time, hand_bolt)?;
                record_pose_eval(&mut self.perf, pose_started);
                let mut hilt_draws = self.render_glm_surfaces(
                    0,
                    &definition.model,
                    &hilt.glm,
                    &hilt.gla,
                    &hilt.surfaces,
                    None,
                    &hilt_pose,
                    0,
                    axis,
                    origin,
                    [1.0; 4],
                    None,
                    false,
                )?;
                if self.rt_shadow_casters_enabled {
                    for surface in &mut hilt_draws {
                        if let Some(key) = surface.rt_skinned_key.take() {
                            surface.rt_skinned_key = Some(Arc::<str>::from(format!(
                                "{}#profile-saber{}",
                                key.as_ref(),
                                saber_num,
                            )));
                        }
                    }
                }
                draws.append(&mut hilt_draws);

                // CG_AddSaberBlade-equivalent preview path. Phase 2 attached the
                // hilt but stopped here, which is why the Profile showed a bare
                // handle. Resolve every authored blade bolt exactly as the live
                // player presenter does and hand the resulting blade to the same
                // WeaponFx material/geometry path used in gameplay.
                let client_color = if saber_num == 0 { info.saber_color } else { info.saber2_color };
                for blade_index in 0..definition.num_blades {
                    let tag_name = format!("*blade{}", blade_index + 1);
                    let (blade_bolt, tag_hack) = if let Some(matrix) = model_bolt_matrix_timed(
                        &mut self.perf, &hilt.glm, &hilt.gla, &hilt_pose, &tag_name,
                    )? {
                        (matrix, false)
                    } else {
                        // UI_SaberDrawBlade falls back to *flash for every
                        // blade, not just blade 0, so pre-JKA hilts still preview.
                        let Some(matrix) = model_bolt_matrix_timed(
                            &mut self.perf, &hilt.glm, &hilt.gla, &hilt_pose, "*flash",
                        )? else {
                            continue;
                        };
                        (matrix, true)
                    };
                    let mut origin_model = [blade_bolt[0][3], blade_bolt[1][3], blade_bolt[2][3]];
                    let mut dir_model = normalize_vec3([
                        -blade_bolt[0][0], -blade_bolt[1][0], -blade_bolt[2][0],
                    ]);
                    // Exact stock UI staff tag-hack: when blade2 has no authored
                    // bolt, reverse the *flash direction and offset sixteen units.
                    if tag_hack
                        && blade_index == 1
                        && definition.saber_type.eq_ignore_ascii_case("SABER_STAFF")
                    {
                        dir_model = [-dir_model[0], -dir_model[1], -dir_model[2]];
                        origin_model = [
                            origin_model[0] + dir_model[0] * 16.0,
                            origin_model[1] + dir_model[1] * 16.0,
                            origin_model[2] + dir_model[2] * 16.0,
                        ];
                    }
                    let origin_world = transform_jka_model_point(origin_model, axis, origin);
                    let dir_world = normalize_vec3(transform_jka_model_vector(dir_model, axis));
                    let blade = definition.blade(blade_index);
                    let color = blade_color(info, client_color, &blade, None);
                    let secondary_style = definition.blade_style2_start > 0
                        && blade_index >= definition.blade_style2_start;
                    let trail_style = if secondary_style { definition.trail_style2 } else { definition.trail_style };
                    let no_wall_marks = if secondary_style { definition.no_wall_marks2 } else { definition.no_wall_marks };
                    if let Some(request) = saber_blade_fx_request(
                        origin_world,
                        dir_world,
                        blade.length,
                        blade.length,
                        blade.radius,
                        color,
                        1.0,
                        0,
                        saber_num as u8,
                        blade_index as u8,
                        0,
                        animation_number,
                        false,
                        trail_style,
                        definition.num_blades as u8,
                        definition.no_dlight,
                        no_wall_marks,
                    ) {
                        self.fx_requests.push(request);
                    }
                }
            }
        }

        // The worn jaPRO cosmetics ride the same pose; the caller drains them
        // into the MD3 presenter (see `drain_cosmetic_draws`).
        if cosmetics != 0 {
            let worn = cosmetic_draws_for_mask(
                &mut self.perf,
                &model,
                &pose,
                axis,
                origin,
                cosmetics,
                0,
                [1.0; 4],
                None,
            );
            self.cosmetic_draws.extend(worn);
        }

        apply_profile_studio_light(&mut draws);
        Ok(draws)
    }

    /// TaystJK's Profile saber preview resolves the selected saber block to its
    /// authored saberModel and optional customSkin. Keep that exact data path,
    /// but render it through DinurdoJK's normal Ghoul2/WGPU presenter.
    // Asset-preview entry point that is not wired into the viewer yet.
    #[allow(dead_code)]
    pub fn present_static_saber_preview(
        &mut self,
        saber_name: &str,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        current_time: i32,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let definition = self.saber_definitions.definition_or_default(saber_name);
        let model = self.load_saber_model(&definition)?;
        let pose_started = Instant::now();
        let pose = Ghoul2Animator::new(&model.gla)
            .evaluate_pose_openjk_root(&model.gla, current_time)?;
        record_pose_eval(&mut self.perf, pose_started);
        self.render_glm_surfaces(
            0,
            &definition.model,
            &model.glm,
            &model.gla,
            &model.surfaces,
            None,
            &pose,
            0,
            axis,
            origin,
            [1.0; 4],
            None,
            true,
        )
    }

    /// Developer Asset Viewer GLM path. Player/NPC model.glm files usually
    /// obtain their materials from model_default.skin rather than from the GLM
    /// hierarchy itself. Try that sibling skin automatically. If it is absent
    /// or incomplete, retain renderable geometry with a neutral $whiteimage
    /// material instead of making the model disappear against the black viewer.
    pub fn present_static_glm_preview(
        &mut self,
        entity_num: u16,
        model_qpath: &str,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        rgba: [f32; 4],
        current_time: i32,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let normalized = model_qpath.replace('\\', "/");
        let inferred_skin = normalized
            .rsplit_once('/')
            .filter(|(_, leaf)| leaf.eq_ignore_ascii_case("model.glm"))
            .map(|(folder, _)| format!("{folder}/model_default.skin"));
        let model = self.load_static_glm_with_options(
            model_qpath,
            inferred_skin.as_deref(),
            model_qpath,
            true,
        )?;
        let pose_started = Instant::now();
        let pose = Ghoul2Animator::new(&model.gla).evaluate_pose_openjk_root(&model.gla, current_time)?;
        record_pose_eval(&mut self.perf, pose_started);
        self.render_glm_surfaces(
            entity_num,
            model_qpath,
            &model.glm,
            &model.gla,
            &model.surfaces,
            None,
            &pose,
            0,
            axis,
            origin,
            rgba,
            None,
            true,
        )
    }

    /// OpenJK's `RE_RegisterModels_GetDiskFile` treats `*default.gla` as a
    /// renderer-internal synthetic GLA rather than a VFS path.  Saber/weapon
    /// GLMs commonly reference it, so do the same before touching the PK3 VFS.
    fn load_glm_animation(&mut self, gla_qpath: &str) -> Result<Arc<GlaAnimation>, String> {
        if gla_qpath.eq_ignore_ascii_case(OPENJK_DEFAULT_GLA_NAME) {
            return Ok(Arc::new(openjk_default_gla()?));
        }
        let gla_bytes = self
            .assets
            .read(gla_qpath, MAX_GLA_BYTES)
            .map_err(|error| format!("{gla_qpath}: {error}"))?
            .ok_or_else(|| format!("missing {gla_qpath}"))?
            .bytes;
        Ok(Arc::new(parse_gla(&gla_bytes)?))
    }

    fn load_skin_qpath(&mut self, qpath: &str) -> Result<Vec<jka_assets::skin::SkinSurface>, String> {
        load_skin(qpath, |name| {
            self.assets
                .read(name, MAX_SKIN_BYTES)
                .map_err(|error| error.to_string())
                .map(|asset| asset.map(|asset| asset.bytes))
        })
    }

    fn resolve_surface_material(&mut self, shader_name: &str) -> ResolvedMaterial {
        let shaders = Arc::clone(&self.shaders);
        let layers = material_layers(&shaders, shader_name);
        let (mut texture, mut alpha_mode) = self.load_stage_texture(&layers.base);
        let overlay = layers.overlay.as_ref().map(|stage| {
            let (texture, alpha_mode) = self.load_stage_texture(stage);
            SurfaceOverlay { texture, alpha_mode }
        });
        let stages = layers
            .stages
            .iter()
            .map(|&(stage, mode)| {
                let (texture, _) = self.load_stage_texture(&StageMaterial {
                    image: stage.image.as_str(),
                    clamp: stage.clamp,
                    alpha_mode: mode,
                });
                resolved_stage(stage, mode, texture)
            })
            .collect::<Vec<_>>();
        if let Some(first) = stages.first() {
            texture = first.texture.clone();
            alpha_mode = first.alpha_mode;
        }
        ResolvedMaterial {
            texture,
            alpha_mode,
            entity_tint: layers.entity_tint,
            overlay,
            stages,
            two_sided: layers.two_sided,
        }
    }

    fn load_stage_texture(&mut self, stage: &StageMaterial<'_>) -> (Option<Arc<TextureData>>, DynamicModelAlphaMode) {
        if stage.image.eq_ignore_ascii_case("$whiteimage") {
            return (None, stage.alpha_mode);
        }
        let texture = self
            .textures
            .load(&mut self.assets, stage.image, stage.clamp)
            .map(|index| {
                if !self.texture_arcs.contains_key(&index) {
                    let image = Arc::new(self.textures.images[index].clone());
                    self.texture_arcs.insert(index, image);
                }
                Arc::clone(&self.texture_arcs[&index])
            });
        if texture.is_none() {
            if let Some(warning) = self.textures.warnings.last() {
                rverbose!(1, "PLAYER TEXTURE WARNING: {warning}");
            }
        }
        (texture, stage.alpha_mode)
    }
}

/// `customRGBA` RGB of a player/NPC entity as a 0..1 tint. A state that never
/// carried the field (all four bytes zero) is untinted; servers always send
/// alpha 255 with it, so a real black tint is still honoured.
/// The viewer's state as jaPRO's `CG_Player` reads it (`cg.snap->ps`).
fn viewer_style_from(
    ps: &jka_protocol::server::PlayerState,
    japro: bool,
    style_player: u32,
) -> crate::japro_cg::StyleViewer {
    const PMF_FOLLOW: i32 = 4096;
    const PERS_TEAM: usize = 3;
    let pm_flags = ps.field_i32("pm_flags").unwrap_or(0);
    crate::japro_cg::StyleViewer {
        japro,
        style_player,
        client_num: ps.field_i32("clientNum").unwrap_or(-1),
        dueling: ps.field_i32("duelInProgress").unwrap_or(0) != 0,
        duel_index: ps.field_i32("duelIndex").unwrap_or(-1),
        racemode: japro && ps.stats[crate::japro_cg::STAT_RACEMODE] != 0,
        coop_race: ps.stats[crate::japro_cg::STAT_MOVEMENTSTYLE] == crate::japro_cg::MV_COOP_JKA,
        free_spectator: ps.persistant[PERS_TEAM] == TEAM_SPECTATOR && pm_flags & PMF_FOLLOW == 0,
    }
}

fn player_entity_rgb(entity: &PresentedEntity) -> [f32; 3] {
    custom_rgba_tint(std::array::from_fn(|index| {
        entity.state.field_i32(&format!("customRGBA[{index}]")).unwrap_or(0)
    }))
}

fn custom_rgba_tint(bytes: [i32; 4]) -> [f32; 3] {
    if bytes.iter().all(|&byte| byte == 0) {
        return [1.0; 3];
    }
    [0, 1, 2].map(|index| bytes[index].clamp(0, 255) as f32 / 255.0)
}

/// CG_PlayerSprites' choice of icon, in its order: connection trouble hides
/// everything, a voice command hides the talk balloon.
fn head_sprite_shader(e_flags: i32, voice_chat_active: bool) -> Option<&'static str> {
    const EF_TALK: i32 = 1 << 13;
    const EF_CONNECTION: i32 = 1 << 14;
    if e_flags & EF_CONNECTION != 0 {
        Some("gfx/2d/net")
    } else if voice_chat_active {
        Some("gfx/mp/vchat_icon")
    } else if e_flags & EF_TALK != 0 {
        Some("gfx/mp/chat_icon")
    } else {
        None
    }
}

fn entity_is_mind_tricked(entity: &PresentedEntity, client: i32) -> bool {
    if !(0..64).contains(&client) {
        return false;
    }
    let (field, bit) = if client > 47 {
        ("trickedentindex4", client - 48)
    } else if client > 31 {
        ("trickedentindex3", client - 32)
    } else if client > 15 {
        ("trickedentindex2", client - 16)
    } else {
        ("trickedentindex", client)
    };
    entity.state.field_i32(field).unwrap_or(0) & (1_i32 << bit) != 0
}

/// Ghoul2/GLM triangle order arrives opposite the WGPU player pipeline's
/// `FrontFace::Ccw` convention. Flip each triangle once at submission time so
/// back-face culling exposes the same side of the model that TaystJK/OpenGL
/// renders instead of making the player look inside-out/front-on from behind.

#[cfg(test)]
fn glm_indices_for_wgpu(mut indices: Vec<u32>) -> Vec<u32> {
    for triangle in indices.chunks_exact_mut(3) {
        triangle.swap(1, 2);
    }
    indices
}

fn transform_model_point(point: [f32; 3], axis: [[f32; 3]; 3], origin: [f32; 3]) -> [f32; 3] {
    let world = [
        origin[0] + axis[0][0] * point[0] + axis[1][0] * point[1] + axis[2][0] * point[2],
        origin[1] + axis[0][1] * point[0] + axis[1][1] * point[1] + axis[2][1] * point[2],
        origin[2] + axis[0][2] * point[0] + axis[1][2] * point[1] + axis[2][2] * point[2],
    ];
    scene::render_position(world)
}

fn transform_model_normal(normal: [f32; 3], axis: [[f32; 3]; 3]) -> [f32; 3] {
    let jka = [
        axis[0][0] * normal[0] + axis[1][0] * normal[1] + axis[2][0] * normal[2],
        axis[0][1] * normal[0] + axis[1][1] * normal[1] + axis[2][1] * normal[2],
        axis[0][2] * normal[0] + axis[1][2] * normal[1] + axis[2][2] * normal[2],
    ];
    let render = scene::render_position(jka);
    let length = (render[0] * render[0] + render[1] * render[1] + render[2] * render[2]).sqrt();
    if length > 0.0 {
        [render[0] / length, render[1] / length, render[2] / length]
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// Build the exact entityState subset consumed by OpenJK BG_G2PlayerAngles.
fn player_angle_entity(entity: &PresentedEntity) -> PlayerAngleEntity {
    PlayerAngleEntity {
        number: i32::from(entity.number),
        entity_type: entity.entity_type,
        velocity: [
            entity.state.field_f32("pos.trDelta[0]").unwrap_or(0.0),
            entity.state.field_f32("pos.trDelta[1]").unwrap_or(0.0),
            entity.state.field_f32("pos.trDelta[2]").unwrap_or(0.0),
        ],
        movement_dir: entity.state.field_f32("angles2[1]").unwrap_or(0.0) as i32,
        legs_anim: entity.state.field_i32("legsAnim").unwrap_or(0),
        torso_anim: entity.state.field_i32("torsoAnim").unwrap_or(0),
        e_flags: entity.state.field_i32("eFlags").unwrap_or(0),
        weapon: entity.state.field_i32("weapon").unwrap_or(0),
        ground_entity_num: entity.state.field_i32("groundEntityNum").unwrap_or(0),
        force_frame: entity.state.field_i32("forceFrame").unwrap_or(0),
        saber_move: entity.state.field_i32("saberMove").unwrap_or(0),
        vehicle_num: entity.state.field_i32("m_iVehicleNum").unwrap_or(0),
        held_by_client: entity.state.field_i32("heldByClient").unwrap_or(0),
        other_entity_num2: entity.state.field_i32("otherEntityNum2").unwrap_or(0),
    }
}

fn openjk_look_target_origin(
    entity: &PresentedEntity,
    entity_origins: &HashMap<u16, [f32; 3]>,
) -> Option<[f32; 3]> {
    if entity.state.field_i32("hasLookTarget").unwrap_or(0) == 0 {
        return None;
    }
    let target = u16::try_from(entity.state.field_i32("lookTarget")?).ok()?;
    entity_origins.get(&target).copied()
}

/// Exact q_math.c `vectoangles` convention used by CG_Player look targets.
pub(super) fn openjk_vectoangles(value: [f32; 3]) -> [f32; 3] {
    let (yaw, pitch) = if value[1] == 0.0 && value[0] == 0.0 {
        (0.0, if value[2] > 0.0 { 90.0 } else { 270.0 })
    } else {
        let mut yaw = if value[0] != 0.0 {
            value[1].atan2(value[0]).to_degrees()
        } else if value[1] > 0.0 {
            90.0
        } else {
            270.0
        };
        if yaw < 0.0 {
            yaw += 360.0;
        }
        let forward = (value[0] * value[0] + value[1] * value[1]).sqrt();
        let mut pitch = value[2].atan2(forward).to_degrees();
        if pitch < 0.0 {
            pitch += 360.0;
        }
        (yaw, pitch)
    };
    [-pitch, yaw, 0.0]
}

fn openjk_player_entity_needs_reset(
    previous_e_flags: i32,
    previous_client_num: i32,
    e_flags: i32,
    client_num: i32,
    force_reset: bool,
    time_rewound: bool,
) -> bool {
    force_reset
        || time_rewound
        || previous_client_num != client_num
        || ((previous_e_flags ^ e_flags) & EF_TELEPORT_BIT) != 0
}

fn flatten_matrix3x4(matrix: &Matrix3x4) -> [f32; 12] {
    [
        matrix[0][0], matrix[0][1], matrix[0][2], matrix[0][3],
        matrix[1][0], matrix[1][1], matrix[1][2], matrix[1][3],
        matrix[2][0], matrix[2][1], matrix[2][2], matrix[2][3],
    ]
}

fn presented_entity_velocity(entity: &PresentedEntity) -> [f32; 3] {
    [
        entity.state.field_f32("pos.trDelta[0]").unwrap_or(0.0),
        entity.state.field_f32("pos.trDelta[1]").unwrap_or(0.0),
        entity.state.field_f32("pos.trDelta[2]").unwrap_or(0.0),
    ]
}

/// Exact OpenJK BG_InKnockDownOnly semantic: the five authored knockdown
/// animations, not getups, rolls, falls, generic velocity or acceleration.
fn presented_entity_in_knockdown(entity: &PresentedEntity) -> bool {
    let animation = entity.state.field_i32("legsAnim").unwrap_or(0) & !ANIM_TOGGLEBIT;
    matches!(
        AnimationSet::name(animation),
        Some(
            "BOTH_KNOCKDOWN1"
                | "BOTH_KNOCKDOWN2"
                | "BOTH_KNOCKDOWN3"
                | "BOTH_KNOCKDOWN4"
                | "BOTH_KNOCKDOWN5"
        )
    )
}

fn impulse_weapon_is_explosive(weapon: i32, alt_fire: bool) -> bool {
    const WP_REPEATER: i32 = 8;
    const WP_FLECHETTE: i32 = 10;
    const WP_ROCKET_LAUNCHER: i32 = 11;
    const WP_THERMAL: i32 = 12;
    const WP_TRIP_MINE: i32 = 13;
    const WP_DET_PACK: i32 = 14;
    const WP_CONCUSSION: i32 = 15;
    matches!(
        weapon,
        WP_ROCKET_LAUNCHER | WP_THERMAL | WP_TRIP_MINE | WP_DET_PACK | WP_CONCUSSION
    ) || (alt_fire && matches!(weapon, WP_REPEATER | WP_FLECHETTE))
}

fn force_grip_target_alive(entity: &PresentedEntity) -> bool {
    (entity.entity_type == ET_PLAYER || entity.entity_type == ET_NPC)
        && entity.state.field_i32("eFlags").unwrap_or(0) & (EF_DEAD | EF_NODRAW) == 0
}

fn force_grip_target_still_valid(gripper: &PresentedEntity, target: &PresentedEntity) -> bool {
    if gripper.number == target.number || !force_grip_target_alive(target) {
        return false;
    }
    let delta = Vec3::from_array(target.origin) - Vec3::from_array(gripper.origin);
    delta.length_squared() <= (MAX_GRIP_DISTANCE + 16.0).powi(2)
}

/// Best-effort client reconstruction of OpenJK ForceGrip's initial trace. The
/// actual target id is intentionally not part of vanilla entityState, so this
/// cannot test BSP occlusion; it selects the first living player/NPC-sized
/// target intersecting the gripper's 256-unit view ray.
fn infer_force_grip_target<'a>(
    gripper: &PresentedEntity,
    candidates: &[&'a PresentedEntity],
) -> Option<&'a PresentedEntity> {
    let start = Vec3::from_array(gripper.origin) + Vec3::Z * DEFAULT_VIEWHEIGHT;
    let forward = Vec3::from_array(angles_to_axis(gripper.angles)[0]).normalize_or_zero();
    if forward.length_squared() < 1.0e-8 {
        return None;
    }

    candidates
        .iter()
        .copied()
        .filter(|target| gripper.number != target.number && force_grip_target_alive(target))
        .filter_map(|target| {
            // OpenJK traces MASK_PLAYERSOLID against the target collision box.
            // We can reproduce that player-box intersection from snapshot data;
            // only BSP/world occlusion is unavailable on this presentation path.
            ray_default_player_box_entry(start, forward, target.origin, MAX_GRIP_DISTANCE)
                .map(|entry| (entry, target))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, target)| target)
}

fn ray_default_player_box_entry(
    start: Vec3,
    direction: Vec3,
    origin: [f32; 3],
    max_distance: f32,
) -> Option<f32> {
    let origin = Vec3::from_array(origin);
    let mins = origin + Vec3::from_array(DEFAULT_PLAYER_MINS);
    let maxs = origin + Vec3::from_array(DEFAULT_PLAYER_MAXS);
    let mut t_min: f32 = 0.0;
    let mut t_max = max_distance;

    for axis in 0..3 {
        let start_axis = start[axis];
        let dir_axis = direction[axis];
        if dir_axis.abs() < 1.0e-6 {
            if start_axis < mins[axis] || start_axis > maxs[axis] {
                return None;
            }
            continue;
        }
        let inv = 1.0 / dir_axis;
        let mut near = (mins[axis] - start_axis) * inv;
        let mut far = (maxs[axis] - start_axis) * inv;
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        t_min = t_min.max(near);
        t_max = t_max.min(far);
        if t_min > t_max {
            return None;
        }
    }

    (t_max >= 0.0 && t_min <= max_distance).then_some(t_min.max(0.0))
}

/// OpenJK/Q3 `AnglesToAxis`: `AngleVectors` followed by negating the right
/// vector so axis[1] is model-left. Retained for regression tests.
fn angles_to_axis(angles: [f32; 3]) -> [[f32; 3]; 3] {
    let (sp, cp) = angles[0].to_radians().sin_cos();
    let (sy, cy) = angles[1].to_radians().sin_cos();
    let (sr, cr) = angles[2].to_radians().sin_cos();

    let forward = [cp * cy, cp * sy, -sp];
    let right = [
        -sr * sp * cy + cr * sy,
        -sr * sp * sy - cr * cy,
        -sr * cp,
    ];
    let up = [
        cr * sp * cy + sr * sy,
        cr * sp * sy - sr * cy,
        cr * cp,
    ];
    [forward, [-right[0], -right[1], -right[2]], up]
}

#[cfg(test)]
mod head_sprite_tests {
    use super::head_sprite_shader;

    #[test]
    fn head_sprite_follows_cg_playersprites_precedence() {
        const TALK: i32 = 1 << 13;
        const CONNECTION: i32 = 1 << 14;
        assert_eq!(head_sprite_shader(0, false), None);
        assert_eq!(head_sprite_shader(TALK, false), Some("gfx/mp/chat_icon"));
        assert_eq!(head_sprite_shader(TALK, true), Some("gfx/mp/vchat_icon"));
        assert_eq!(head_sprite_shader(TALK | CONNECTION, true), Some("gfx/2d/net"));
        assert_eq!(head_sprite_shader(0, true), Some("gfx/mp/vchat_icon"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for_player_model(
        presenter: &mut PlayerPresenter,
        info: &crate::cgame::ClientInfo,
    ) -> ModelResolution {
        let deadline = Instant::now() + std::time::Duration::from_secs(60);
        loop {
            presenter.poll_asset_completions();
            match presenter.resolve_player_model_async(info) {
                ModelResolution::Loading | ModelResolution::Provisional(_) => {
                    assert!(Instant::now() < deadline, "player model did not finish loading");
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                done => return done,
            }
        }
    }

    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock PK3s"]
    fn async_player_model_registration_never_blocks_and_deduplicates() {
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let mut presenter = PlayerPresenter::new(assets, false).unwrap();
        let queued_after_prime = presenter.async_models.stats().queued;

        // Uncached model: the request returns without waiting for IO/parsing.
        let reborn = crate::cgame::ClientInfo::solo_model("reborn/default");
        let started = Instant::now();
        let first = presenter.resolve_player_model_async(&reborn);
        assert!(started.elapsed().as_millis() < 50, "request blocked the caller");
        assert!(!matches!(first, ModelResolution::Ready(_)));
        for _ in 0..25 {
            presenter.resolve_player_model_async(&reborn);
        }
        let stats = presenter.async_models.stats();
        assert!(stats.duplicates_avoided >= 25, "{stats:?}");

        // The requested model swaps in when ready and was queued exactly once.
        let ModelResolution::Ready(model) = wait_for_player_model(&mut presenter, &reborn) else {
            panic!("reborn should load");
        };
        assert!(model.key.starts_with("models/players/reborn/"));
        let queued_for_reborn = presenter.async_models.stats().queued - queued_after_prime;
        assert!(queued_for_reborn <= 1, "reborn queued {queued_for_reborn} times");

        // A second consumer resolves from the cache with no new job.
        let queued_before = presenter.async_models.stats().queued;
        assert!(matches!(
            presenter.resolve_player_model_async(&reborn),
            ModelResolution::Ready(_)
        ));
        assert_eq!(presenter.async_models.stats().queued, queued_before);

        // Missing model: terminal failure -> OpenJK fallback, never re-queued.
        let missing = crate::cgame::ClientInfo::solo_model("definitely_not_a_model/default");
        let ModelResolution::Ready(fallback) = wait_for_player_model(&mut presenter, &missing) else {
            panic!("missing model should fall back to the default body");
        };
        assert!(fallback.key.starts_with("models/players/kyle/"));
        let queued_before = presenter.async_models.stats().queued;
        for _ in 0..50 {
            presenter.resolve_player_model_async(&missing);
        }
        assert_eq!(presenter.async_models.stats().queued, queued_before);

        // Shared skeleton: every humanoid model resolves the same GLA Arc, so a
        // model swap can keep the animation state.
        assert!(Arc::ptr_eq(&model.gla, &fallback.gla));
    }

    fn tinted_test_shaders() -> BTreeMap<String, Shader> {
        jka_assets::shader::parse(
            "models/players/test/torso_04_clothes {
{
map models/players/test/torso_04
blendFunc GL_ONE GL_ZERO
rgbGen lightingDiffuseEntity
}
{
map models/players/test/torso_04
blendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA
detail
rgbGen lightingDiffuse
}
}
models/players/test/plain {
{
map models/players/test/plain
blendFunc GL_ONE GL_ZERO
rgbGen lightingDiffuse
}
{
map models/players/test/plain
blendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA
}
}
",
        )
        .unwrap()
    }

    #[test]
    fn skin_shader_names_with_an_extension_resolve_the_entity_tint_layers() {
        let shaders = tinted_test_shaders();
        // The skin entry carries `.tga`; R_FindShader strips it.
        let layers = material_layers(&shaders, "models/players/test/torso_04_clothes.tga");
        assert_eq!(layers.base.image, "models/players/test/torso_04");
        // GL_ONE GL_ZERO replaces the framebuffer, so the texture alpha mask is not blended.
        assert_eq!(layers.base.alpha_mode, DynamicModelAlphaMode::Opaque);
        assert!(layers.entity_tint);
        let overlay = layers.overlay.expect("untinted alpha-blended overlay stage");
        assert_eq!(overlay.image, "models/players/test/torso_04");
        assert_eq!(overlay.alpha_mode, DynamicModelAlphaMode::Blend);
    }

    /// GlowTink (TinkerBell 1.0): a `GL_SRC_ALPHA GL_ONE` first stage driven by
    /// `alphaGen lightingSpecular` must not be drawn at full alpha (it washed the
    /// model white); the glow stages after it carry the look.
    #[test]
    fn blended_glow_shaders_draw_every_supported_stage() {
        let shaders = jka_assets::shader::parse(
            "models/players/GlowTink/tink3
{
    cull twosided
    {
        map models/players/GlowTink/tink3
        blendFunc GL_SRC_ALPHA GL_ONE
        alphaGen lightingSpecular
        tcMod scale 2.2 2.2
        tcMod scroll 0.0 -1.01
    }
    {
        map models/players/GlowTink/tink3_e
        blendFunc GL_ONE GL_ONE
        rgbGen const ( 1.4 1.2 0.5 )
        tcMod scale 2.2 2.2
        tcMod scroll 0.0 -1.01
    }
    {
        map models/players/GlowTink/tink3_cel
        blendFunc GL_DST_COLOR GL_ONE
        rgbGen const ( 1.0 0.9 0.2 )
        tcGen environment
    }
    {
        map models/players/GlowTink/tink3_e
        blendFunc GL_ONE GL_ONE
        rgbGen const ( 2.2 1.8 0.7 )
        tcMod scale 4.2 4.2
    }
}
models/players/GlowTink/black
{
    {
        map models/players/GlowTink/black
        blendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA
        rgbGen const ( 1.18 1.18 1.18 )
    }
    {
        map models/players/GlowTink/tink3_e
        blendFunc GL_ONE GL_ONE
        rgbGen const ( 1.2 1.0 0.45 )
    }
}
",
        )
        .expect("test shaders parse");
        let layers = material_layers(&shaders, "models/players/GlowTink/tink3.tga");
        let stages = layers
            .stages
            .iter()
            .map(|(stage, mode)| (stage.image.as_str(), *mode))
            .collect::<Vec<_>>();
        assert_eq!(
            stages,
            [
                ("models/players/glowtink/tink3", DynamicModelAlphaMode::Additive),
                ("models/players/glowtink/tink3_e", DynamicModelAlphaMode::AdditiveOne),
                ("models/players/glowtink/tink3_e", DynamicModelAlphaMode::AdditiveOne),
            ],
            "the GL_DST_COLOR environment filter is not drawn"
        );
        assert!(resolved_stage(layers.stages[0].0, layers.stages[0].1, None).specular_alpha);
        let glow = resolved_stage(layers.stages[1].0, layers.stages[1].1, None);
        // The shader parser clamps rgbGen const to 0..1.
        assert_eq!(glow.rgb, [1.0, 1.0, 0.5]);
        assert!(glow.unlit);

        let layers = material_layers(&shaders, "models/players/GlowTink/black");
        let modes = layers.stages.iter().map(|(_, mode)| *mode).collect::<Vec<_>>();
        assert_eq!(modes, [DynamicModelAlphaMode::BlendUnlit, DynamicModelAlphaMode::AdditiveOne]);
    }

    #[test]
    fn specular_alpha_peaks_on_the_mirror_direction_and_clamps_backfacing_to_zero() {
        // JKA (0,0,1) is render (0,1,0). Light straight above, viewer straight above:
        // the reflection of the light is the viewer direction.
        let (position, up) = ([0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let alpha = specular_alpha(position, up, [0.0, 0.0, 100.0], [0.0, 0.0, 100.0]);
        assert!((alpha - 1.0).abs() < 1.0e-5, "{alpha}");
        // A viewer below the surface sees no glint.
        assert_eq!(specular_alpha(position, up, [0.0, 0.0, 100.0], [0.0, 0.0, -100.0]), 0.0);
    }

    #[test]
    fn stage_tc_mods_compose_in_order_and_scroll_wraps() {
        let mods = [TcMod::Scale(2.0, 2.0), TcMod::Scroll(0.0, -0.5)];
        assert_eq!(stage_uv_xform(&mods, 0.0), [2.0, 2.0, 0.0, 0.0]);
        // A quarter second scrolls -0.125, wrapped into [0, 1).
        assert_eq!(stage_uv_xform(&mods, 0.25), [2.0, 2.0, 0.0, 0.875]);
        // Scaling after a scroll scales the offset with it.
        let mods = [TcMod::Scroll(0.25, 0.0), TcMod::Scale(4.0, 1.0)];
        assert_eq!(stage_uv_xform(&mods, 1.0 / 8.0), [4.0, 1.0, 0.125, 0.0]);
    }

    #[test]
    fn opaque_base_player_shaders_keep_the_single_stage_path() {
        let shaders = jka_assets::shader::parse(
            "models/players/TinkerBell/newer
{
    cull disable
    {
        map models/players/TinkerBell/newer
        alphaFunc GE128
        rgbGen lightingDiffuse
    }
    {
        map models/players/TinkerBell/newer_s
        blendFunc GL_ONE GL_ONE
        rgbGen identity
        alphaGen lightingSpecular
    }
}
",
        )
        .expect("test shaders parse");
        let layers = material_layers(&shaders, "models/players/TinkerBell/newer");
        assert!(layers.stages.is_empty());
        assert_eq!(layers.base.alpha_mode, DynamicModelAlphaMode::Mask);
    }

    #[test]
    fn shaders_without_an_entity_colour_stage_stay_single_pass() {
        let shaders = tinted_test_shaders();
        let layers = material_layers(&shaders, "models/players/test/plain");
        assert!(!layers.entity_tint);
        assert!(layers.overlay.is_none());
        // A name with no shader is a plain texture path.
        let layers = material_layers(&shaders, "models/players/test/kyle.tga");
        assert_eq!(layers.base.image, "models/players/test/kyle.tga");
        assert!(!layers.entity_tint);
    }

    #[test]
    fn custom_rgba_tints_players_and_missing_state_is_untinted() {
        assert_eq!(custom_rgba_tint([0; 4]), [1.0; 3]);
        assert_eq!(custom_rgba_tint([255, 255, 255, 255]), [1.0; 3]);
        assert_eq!(custom_rgba_tint([255, 0, 51, 255]), [1.0, 0.0, 0.2]);
        // A genuine black tint carries alpha 255 and must not read as "unset".
        assert_eq!(custom_rgba_tint([0, 0, 0, 255]), [0.0; 3]);
    }

    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock PK3s"]
    fn jedi_zf_torso_registers_with_entity_tint_and_overlay() {
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let mut presenter = PlayerPresenter::new(assets, false).unwrap();
        let info = crate::cgame::ClientInfo::solo_model("jedi_zf/default");
        let model = presenter.load_model(&info).unwrap();
        let tinted: Vec<_> = model.surfaces.iter().filter(|surface| surface.entity_tint).collect();
        assert!(!tinted.is_empty(), "jedi_zf default skin has an lightingDiffuseEntity surface");
        for surface in tinted {
            assert!(surface.texture.is_some(), "tinted surface lost its texture (white torso)");
            assert_eq!(surface.alpha_mode, DynamicModelAlphaMode::Opaque);
            assert!(surface.overlay.as_ref().is_some_and(|overlay| overlay.texture.is_some()));
        }
        assert!(model.surfaces.iter().all(|surface| surface.texture.is_some()));
    }

    /// ShadowTink (TinkerBell 1.0): an opaque `rgbGen const` base under scrolling
    /// marble/smoke layers, a lit opaque eye under a `glow` map, and twosided
    /// additive wings. Each was drawn as its primary stage only.
    #[test]
    fn opaque_base_shaders_with_overlay_stages_draw_the_whole_stack() {
        let shaders = jka_assets::shader::parse(
            "models/players/ShadowTink/legs
{
    cull twosided
    {
        map models/players/ShadowTink/legs
        blendFunc GL_ONE GL_ZERO
        rgbGen const ( 0.2 0.0 0.4 )
    }
    {
        map models/players/ShadowTink/tink43
        blendFunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA
        rgbGen const ( 0.6 0.6 0.6 )
        alphaGen const 0.45
        tcMod scale 2.2 2.2
    }
}
models/players/ShadowTink/ShadowEyes
{
    {
        map models/players/ShadowTink/ShadowEyes
        blendFunc GL_ONE GL_ZERO
        rgbGen lightingDiffuse
    }
    {
        map models/players/ShadowTink/ShadowEyes_g
        blendFunc GL_ONE GL_ONE
        detail
        glow
    }
}
models/players/ShadowTink/ShadowWings
{
    cull twosided
    {
        map models/players/ShadowTink/ShadowWings
        blendFunc GL_ONE GL_ONE
        rgbGen wave sin 2.5 2.5 0 0
    }
}
models/players/test/plain_opaque
{
    {
        map models/players/test/plain_opaque
        rgbGen lightingDiffuse
    }
}
",
        )
        .expect("test shaders parse");

        let legs = material_layers(&shaders, "models/players/ShadowTink/legs.tga");
        let modes = legs.stages.iter().map(|(_, mode)| *mode).collect::<Vec<_>>();
        assert_eq!(modes, [DynamicModelAlphaMode::Opaque, DynamicModelAlphaMode::BlendUnlit]);
        let base = resolved_stage(legs.stages[0].0, legs.stages[0].1, None);
        assert_eq!(base.rgb, [0.2, 0.0, 0.4]);
        assert!(base.unlit, "rgbGen const is not lit by the light grid");
        assert!(!legs.two_sided, "an opaque body hides its own back faces");

        let eyes = material_layers(&shaders, "models/players/ShadowTink/ShadowEyes");
        let modes = eyes.stages.iter().map(|(_, mode)| *mode).collect::<Vec<_>>();
        assert_eq!(modes, [DynamicModelAlphaMode::Opaque, DynamicModelAlphaMode::AdditiveOne]);
        assert!(!resolved_stage(eyes.stages[0].0, eyes.stages[0].1, None).unlit);

        let wings = material_layers(&shaders, "models/players/ShadowTink/ShadowWings");
        assert!(wings.two_sided, "twosided additive wings need reversed triangles");

        let plain = material_layers(&shaders, "models/players/test/plain_opaque");
        assert!(plain.stages.is_empty() && !plain.two_sided);
    }

    #[test]
    fn body_queue_weapon_rule_copies_actual_model_then_applies_known_weapon() {
        // CG_BodyQueueCopy never manufactures model index 1 when the source
        // Ghoul2 did not actually have one.
        assert_eq!(body_queue_model1_weapon(None, WP_SABER), None);
        // A normal saber death keeps model index 1 occupied, but OpenJK's
        // ET_BODY replacement resolves WP_SABER through the default weapon
        // instance rather than the live client's custom primary saber.
        assert_eq!(body_queue_model1_weapon(Some(WP_SABER), WP_SABER), Some(WP_SABER));
        // Weapons newer than Bryar are stripped from model index 1. Their
        // separately dropped ET_ITEM, when the server creates one, owns them.
        assert_eq!(body_queue_model1_weapon(Some(WP_BRYAR_PISTOL + 1), WP_BRYAR_PISTOL + 1), None);
        // Low-tier known weapons replace an already-existing duplicated slot.
        assert_eq!(
            body_queue_model1_weapon(Some(WP_SABER), WP_BRYAR_PISTOL),
            Some(WP_BRYAR_PISTOL),
        );
    }

    #[test]
    fn openjk_dead_saber_length_retracts_toward_zero() {
        let mut blade = SaberBladeLengthState::new(40.0, -1.0, 1000);
        assert_eq!(blade.length, 40.0);
        let first = blade.set_desired_and_update(0.0, 40.0, 1016);
        assert!(first < 40.0 && first > 0.0);
        let later = blade.set_desired_and_update(0.0, 40.0, 1100);
        assert!(later < first);
    }

    #[test]
    fn zero_length_saber_request_reaches_weapon_fx_cleanup() {
        let request = saber_blade_fx_request(
            [0.0; 3], [1.0, 0.0, 0.0], 0.0, 40.0, 3.0, 4, 1.0, 7, 0, 0, 0, 0,
            false, 0, 1, false, false,
        )
        .expect("zero-length terminal sample must reach WeaponFx cleanup");
        let PlayerFxRequest::SaberBlade { length, .. } = request else {
            panic!("expected saber blade request");
        };
        assert_eq!(length, 0.0);
    }

    #[test]
    fn force_grip_trace_hits_default_player_box() {
        let start = Vec3::new(0.0, 0.0, DEFAULT_VIEWHEIGHT);
        let forward = Vec3::X;
        let hit = ray_default_player_box_entry(
            start,
            forward,
            [128.0, 0.0, 0.0],
            MAX_GRIP_DISTANCE,
        );
        assert!(hit.is_some());
        assert!((hit.unwrap() - 113.0).abs() < 1.0e-4);
    }

    #[test]
    fn force_grip_trace_misses_off_axis_player_box() {
        let start = Vec3::new(0.0, 0.0, DEFAULT_VIEWHEIGHT);
        let forward = Vec3::X;
        assert!(ray_default_player_box_entry(
            start,
            forward,
            [128.0, 80.0, 0.0],
            MAX_GRIP_DISTANCE,
        )
        .is_none());
    }

    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock PK3s and demos/cheezyVsource.dm_26"]
    fn demo_saber_throw_rotates_returns_and_catches() {
        use jka_protocol::{demo::DemoReader, server::{Decoder, Event}};
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let bytes = assets.read("demos/cheezyVsource.dm_26", 64 * 1024 * 1024)
            .unwrap().expect("regression demo").bytes;
        let mut presenter = PlayerPresenter::new(assets, false).unwrap();
        presenter.set_async_loading(false);
        let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
        let mut decoder = Decoder::new();
        let mut game = ClientGameState::new();
        let mut directions: HashMap<u16, [f32; 3]> = HashMap::new();
        let mut spinning = false;
        let mut returning = false;
        let mut caught = false;
        let mut frames = 0;
        while let Some(record) = reader.next_record().unwrap() {
            let packet = decoder.parse_packet(record.sequence, &record.payload).unwrap();
            for event in packet.events {
                match event {
                    Event::Gamestate { server_command_sequence, .. } => {
                        game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                    }
                    Event::ServerCommand(command) => game.queue_server_command(command),
                    Event::Snapshot { .. } => {
                        let snapshot = decoder.latest_snapshot().unwrap();
                        game.set_initial_snapshot(snapshot).unwrap();
                        let mut entities = game.present_entities(snapshot.server_time).unwrap();
                        if let Some(followed) = game.present_followed_player(snapshot.server_time) {
                            entities.push(followed);
                        }
                        for owner in entities.iter().filter(|entity| entity.entity_type == ET_PLAYER) {
                            let client = owner.state.field_i32("clientNum").unwrap() as usize;
                            let Some(info) = game.client_info(client, &[]) else { continue; };
                            let was_flying = presenter.thrown_sabers.contains_key(&owner.number);
                            let draws = presenter.present_thrown_saber_for_player(
                                owner, &info, &entities, &game, snapshot.server_time, 1.0,
                            ).unwrap();
                            if owner.state.field_i32("saberInFlight") == Some(0) {
                                if was_flying {
                                    assert!(draws.is_empty());
                                    assert!(!presenter.thrown_sabers.contains_key(&owner.number));
                                    caught = true;
                                }
                                continue;
                            }
                            if draws.is_empty() { continue; }
                            frames += 1;
                            let saber_num = owner.state.field_i32("saberEntityNum").unwrap() as u16;
                            let saber = entities.iter().find(|entity| entity.number == saber_num).unwrap();
                            assert_eq!(saber.state.field_i32("apos.trType"), Some(super::super::TR_LINEAR));
                            let def = presenter.saber_definitions.definition_or_default(&info.saber_name);
                            let angles = presenter.thrown_sabers.get_mut(&owner.number).unwrap().angles(
                                owner, saber, snapshot.server_time, snapshot.message_num, def.return_damage,
                            ).unwrap();
                            if saber.state.field_i32("saberInFlight") == Some(1) && angles[0] == 90.0 {
                                let hilt = presenter.load_saber_model(&def).unwrap();
                                let pose = Ghoul2Animator::new(&hilt.gla)
                                    .evaluate_pose_openjk_root(&hilt.gla, snapshot.server_time).unwrap();
                                let bolt = model_bolt_matrix(&hilt.glm, &hilt.gla, &pose, "*blade1")
                                    .unwrap().expect("staff blade bolt");
                                let dir = normalize_vec3(transform_jka_model_vector(
                                    [-bolt[0][0], -bolt[1][0], -bolt[2][0]],
                                    angles_to_axis(blade_angles(angles)),
                                ));
                                assert!(dir[2].abs() < 0.05, "settled blade must be horizontal: {dir:?}");
                                if let Some(previous) = directions.insert(owner.number, dir) {
                                    let dot: f32 = previous.iter().zip(dir).map(|(a, b)| a * b).sum();
                                    spinning |= dot < 0.99;
                                }
                            } else if saber.state.field_i32("saberInFlight") == Some(0) {
                                let mut expected = openjk_vectoangles([
                                    saber.origin[0] - owner.origin[0], saber.origin[1] - owner.origin[1],
                                    saber.origin[2] - owner.origin[2],
                                ]);
                                expected[0] += 90.0;
                                if was_flying && !def.return_damage && saber.state.field_i32("bolt2") != Some(123) {
                                    for axis in 0..3 { assert!((angles[axis] - expected[axis]).abs() < 0.001); }
                                    returning = true;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            if spinning && returning && caught { break; }
        }
        println!("SABER REGRESSION frames={frames} spinning={spinning} returning={returning} caught={caught}");
        assert!(spinning && returning && caught);
    }

    /// sabers.dm_26: a staff (`dual_3`) thrown in single-blade mode
    /// (saberHolstered == 1) must fly with only its first blade, and a second
    /// saber selected at runtime (`saber kyle kyle`) must be attached in hand as
    /// soon as the userinfo changes, not only after the primary is thrown and
    /// caught.
    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock PK3s and demos/sabers.dm_26"]
    fn demo_runtime_saber_changes_and_half_staff_throw() {
        use jka_protocol::{demo::DemoReader, server::{Decoder, Event}};
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let bytes = assets.read("demos/sabers.dm_26", 64 * 1024 * 1024)
            .unwrap().expect("regression demo").bytes;
        let mut presenter = PlayerPresenter::new(assets, false).unwrap();
        presenter.set_async_loading(false);
        let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
        let mut decoder = Decoder::new();
        let mut game = ClientGameState::new();
        let mut snapshots = 0;
        let (mut half_staff_throw_frames, mut dual_frames) = (0, 0);
        while let Some(record) = reader.next_record().unwrap() {
            let packet = decoder.parse_packet(record.sequence, &record.payload).unwrap();
            for event in packet.events {
                match event {
                    Event::Gamestate { server_command_sequence, .. } => {
                        game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                    }
                    Event::ServerCommand(command) => game.queue_server_command(command),
                    Event::Snapshot { .. } => {
                        let snapshot = decoder.latest_snapshot().unwrap();
                        if snapshots == 0 {
                            game.set_initial_snapshot(snapshot).unwrap();
                        } else {
                            game.set_next_snapshot(Some(snapshot)).unwrap();
                            game.transition_snapshot(snapshot.server_time).unwrap();
                        }
                        snapshots += 1;
                        let mut entities = game.present_entities(snapshot.server_time).unwrap();
                        let Some(followed) = game.present_followed_player(snapshot.server_time) else { continue };
                        entities.push(followed.clone());
                        let client = followed.state.field_i32("clientNum").unwrap_or(0) as usize;
                        let Some(info) = game.client_info(client, &[]) else { continue };
                        presenter.present_snapshot_players(
                            &entities, &game, &[], snapshot.server_time, None, None, None, None,
                        );
                        presenter.drain_fx_requests();
                        let in_flight = followed.state.field_i32("saberInFlight").unwrap_or(0) != 0;
                        let holstered = followed.state.field_i32("saberHolstered").unwrap_or(0);
                        let weapon = followed.state.field_i32("weapon").unwrap_or(0);
                        let [_, has_secondary] = presenter.saber_definitions.equipped_slots(
                            [&info.saber_name, &info.saber2_name], true,
                        );
                        if weapon == WP_SABER && has_secondary && !in_flight {
                            dual_frames += 1;
                            assert!(
                                presenter.entities[&followed.number].secondary_saber_attached,
                                "second saber '{}' not attached at {}", info.saber2_name, snapshot.server_time,
                            );
                        }
                        if in_flight && holstered == 1 && info.saber_name.eq_ignore_ascii_case("dual_3") {
                            presenter.present_thrown_saber_for_player(
                                &followed, &info, &entities, &game, snapshot.server_time, 1.0,
                            ).unwrap();
                            let blades: Vec<u8> = presenter.drain_fx_requests().into_iter().filter_map(|request| match request {
                                PlayerFxRequest::SaberBlade { blade_num, saber_in_flight: true, .. } => Some(blade_num),
                                _ => None,
                            }).collect();
                            if !blades.is_empty() {
                                half_staff_throw_frames += 1;
                                assert!(blades.iter().all(|&blade| blade == 0), "extra blades drawn on a half-staff throw: {blades:?}");
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        println!("SABER SETUP REGRESSION snapshots={snapshots} halfStaffThrowFrames={half_staff_throw_frames} dualFrames={dual_frames}");
        assert!(half_staff_throw_frames > 0, "demo no longer throws a half staff");
        assert!(dual_frames > 0, "demo no longer wields dual sabers");
    }

    /// Reborn NPCs held by a spawner are parked EF_NODRAW with anim 0
    /// (FACE_TALK0, a reference-pose frame); CG_Player must not draw them.
    #[test]
    #[ignore = "requires JKA_TEST_BASE and demos/TEST.dm_26"]
    fn demo_nodraw_npcs_are_not_submitted_in_reference_pose() {
        use jka_protocol::{demo::DemoReader, server::{Decoder, Event}};
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let bytes = assets.read("demos/TEST.dm_26", 64 * 1024 * 1024).unwrap().unwrap().bytes;
        let mut presenter = PlayerPresenter::new(assets, false).unwrap();
        presenter.set_async_loading(false);
        let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
        let mut decoder = Decoder::new();
        let mut game = ClientGameState::new();
        let (mut snapshots, mut nodraw_frames) = (0, 0);
        while let Some(record) = reader.next_record().unwrap() {
            let packet = decoder.parse_packet(record.sequence, &record.payload).unwrap();
            for event in packet.events {
                match event {
                    Event::Gamestate { server_command_sequence, .. } => {
                        game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                    }
                    Event::ServerCommand(command) => game.queue_server_command(command),
                    Event::Snapshot { .. } => {
                        let snapshot = decoder.latest_snapshot().unwrap();
                        if snapshots == 0 {
                            game.set_initial_snapshot(snapshot).unwrap();
                        } else {
                            game.set_next_snapshot(Some(snapshot)).unwrap();
                            game.transition_snapshot(snapshot.server_time).unwrap();
                        }
                        snapshots += 1;
                        let entities = game.present_entities(snapshot.server_time).unwrap();
                        let draws = presenter.present_snapshot_players(&entities, &game, &[], snapshot.server_time, None, None, None, None);
                        for npc in entities.iter().filter(|entity| entity.entity_type == ET_NPC) {
                            let drawn = draws.iter().any(|surface| surface.entity_num == npc.number);
                            let nodraw = npc.state.field_i32("eFlags").unwrap_or(0) & EF_NODRAW != 0;
                            if nodraw {
                                nodraw_frames += 1;
                                assert!(!drawn, "EF_NODRAW NPC {} submitted at {}", npc.number, snapshot.server_time);
                            } else {
                                assert_ne!(npc.state.field_i32("legsAnim").unwrap_or(0) & !0x800, 0, "visible NPC {} in anim 0", npc.number);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        println!("NODRAW REGRESSION snapshots={snapshots} nodrawNpcFrames={nodraw_frames}");
        assert!(nodraw_frames > 0, "demo no longer exercises spawner-held NPCs");
    }

    /// The TEST.dm_26 recorder switches between the saber and blaster,
    /// repeater, rocket launcher, thermal and concussion rifle: each non-saber
    /// weapon's world GLM must be bolted to *r_hand and submitted.
    #[test]
    #[ignore = "requires JKA_TEST_BASE and demos/TEST.dm_26"]
    fn demo_followed_player_holds_non_saber_weapons() {
        use jka_protocol::{demo::DemoReader, server::{Decoder, Event}};
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let bytes = assets.read("demos/TEST.dm_26", 64 * 1024 * 1024).unwrap().unwrap().bytes;
        let mut presenter = PlayerPresenter::new(assets, false).unwrap();
        presenter.set_async_loading(false);
        let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
        let mut decoder = Decoder::new();
        let mut game = ClientGameState::new();
        let mut snapshots = 0;
        let mut held: std::collections::BTreeMap<i32, usize> = Default::default();
        while let Some(record) = reader.next_record().unwrap() {
            let packet = decoder.parse_packet(record.sequence, &record.payload).unwrap();
            for event in packet.events {
                match event {
                    Event::Gamestate { server_command_sequence, .. } => {
                        game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                    }
                    Event::ServerCommand(command) => game.queue_server_command(command),
                    Event::Snapshot { .. } => {
                        let snapshot = decoder.latest_snapshot().unwrap();
                        if snapshots == 0 {
                            game.set_initial_snapshot(snapshot).unwrap();
                        } else {
                            game.set_next_snapshot(Some(snapshot)).unwrap();
                            game.transition_snapshot(snapshot.server_time).unwrap();
                        }
                        snapshots += 1;
                        let Some(followed) = game.present_followed_player(snapshot.server_time) else { continue };
                        let client = followed.state.field_i32("clientNum").unwrap_or(0) as usize;
                        let Some(info) = game.client_info(client, &[]) else { continue };
                        let draws = presenter
                            .present_player_entity(&followed, &info, snapshot.server_time, 1.0, None, false, true, false, None)
                            .unwrap();
                        let weapon = followed.state.field_i32("weapon").unwrap_or(0);
                        let attached = presenter.entities[&followed.number].attached_weapon;
                        if weapon != 0 && weapon != WP_SABER && weapon != WP_MELEE {
                            assert_eq!(attached, Some(weapon), "weapon {weapon} not bolted at {}", snapshot.server_time);
                            let body = presenter.models[&presenter.entities[&followed.number].model_key].surfaces.len();
                            assert!(draws.len() > body, "weapon {weapon} surfaces not submitted");
                            *held.entry(weapon).or_insert(0) += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
        println!("HELD WEAPON REGRESSION snapshots={snapshots} held={held:?}");
        assert!(held.len() >= 3, "demo exercised too few held weapons: {held:?}");
    }

    /// Force powers in TEST.dm_26: CG_Player's hand-bolt effects, puffs and
    /// shells must reach the FX system and produce draws.
    #[test]
    #[ignore = "requires JKA_TEST_BASE and demos/TEST.dm_26"]
    fn demo_force_powers_produce_fx() {
        use jka_protocol::{demo::DemoReader, server::{Decoder, Event}};
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let open = || AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let bytes = open().read("demos/TEST.dm_26", 64 * 1024 * 1024).unwrap().unwrap().bytes;
        let mut presenter = PlayerPresenter::new(open(), false).unwrap();
        presenter.set_async_loading(false);
        let mut weapon_fx = crate::cgame::weapon_fx::WeaponFx::new(open());
        let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
        let mut decoder = Decoder::new();
        let mut game = ClientGameState::new();
        let mut snapshots = 0;
        let mut requests: std::collections::BTreeMap<String, usize> = Default::default();
        let mut state_counts: std::collections::BTreeMap<&str, usize> = Default::default();
        let (mut fx_draw_frames, mut shell_surfaces) = (0, 0);
        while let Some(record) = reader.next_record().unwrap() {
            let packet = decoder.parse_packet(record.sequence, &record.payload).unwrap();
            for event in packet.events {
                match event {
                    Event::Gamestate { server_command_sequence, .. } => {
                        game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                    }
                    Event::ServerCommand(command) => game.queue_server_command(command),
                    Event::Snapshot { .. } => {
                        let snapshot = decoder.latest_snapshot().unwrap();
                        if snapshots == 0 {
                            game.set_initial_snapshot(snapshot).unwrap();
                        } else {
                            game.set_next_snapshot(Some(snapshot)).unwrap();
                            game.transition_snapshot(snapshot.server_time).unwrap();
                        }
                        snapshots += 1;
                        weapon_fx.begin_frame(snapshot.server_time);
                        let mut entities = game.present_entities(snapshot.server_time).unwrap();
                        if let Some(followed) = game.present_followed_player(snapshot.server_time) {
                            entities.push(followed);
                        }
                        for entity in entities.iter().filter(|e| e.entity_type == ET_PLAYER || e.entity_type == ET_NPC) {
                            let state = &entity.state;
                            if state.field_i32("activeForcePass").unwrap_or(0) != 0 { *state_counts.entry("activeForcePass").or_insert(0) += 1; }
                            if state.field_i32("powerups").unwrap_or(0) & (1 << PW_DISINT_4) != 0 { *state_counts.entry("PW_DISINT_4").or_insert(0) += 1; }
                            if state.field_i32("eFlags").unwrap_or(0) & EF_BODYPUSH != 0 { *state_counts.entry("EF_BODYPUSH").or_insert(0) += 1; }
                            let active = state.field_i32("forcePowersActive").unwrap_or(0);
                            if active & (1 << FP_PROTECT) != 0 { *state_counts.entry("FP_PROTECT").or_insert(0) += 1; }
                            if active & (1 << FP_RAGE) != 0 { *state_counts.entry("FP_RAGE").or_insert(0) += 1; }
                            if active & (1 << FP_GRIP) != 0 { *state_counts.entry("FP_GRIP").or_insert(0) += 1; }
                        }
                        let draws = presenter.present_snapshot_players(&entities, &game, &[], snapshot.server_time, None, None, None, None);
                        shell_surfaces += draws.iter().filter(|surface| surface.alpha_mode == DynamicModelAlphaMode::Additive && surface.vertices.len() > 50).count();
                        for request in presenter.drain_fx_requests() {
                            let key = match &request {
                                PlayerFxRequest::Effect { name, .. } => name.to_string(),
                                PlayerFxRequest::EffectDir { name, .. } => name.clone(),
                                PlayerFxRequest::SaberBlade { .. } => "saber blade".into(),
                                PlayerFxRequest::PushPuffs { .. } => "push puffs".into(),
                                PlayerFxRequest::GripPuffs { .. } => "grip puffs".into(),
                                PlayerFxRequest::HeadSprite { shader, .. } => format!("head sprite {shader}"),
                            };
                            *requests.entry(key).or_insert(0) += 1;
                            weapon_fx.player_fx(&request);
                        }
                        if !weapon_fx.end_frame().draws.is_empty() {
                            fx_draw_frames += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
        println!("FORCE REGRESSION snapshots={snapshots} states={state_counts:?} requests={requests:?} fxDrawFrames={fx_draw_frames} shellSurfaces={shell_surfaces} fx={:?}", weapon_fx.stats());
        assert!(!requests.is_empty(), "no force power visuals were requested");
        assert!(fx_draw_frames > 0);
    }

    /// TEST.dm_26 (mp/ffa3) is full of saber-wielding reborn NPCs: they must
    /// resolve an npcClient, pose through CG_Player and submit body + blades.
    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock PK3s and demos/TEST.dm_26"]
    fn demo_reborn_npcs_submit_body_and_saber_geometry() {
        use jka_protocol::{demo::DemoReader, server::{Decoder, Event}};
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let bytes = assets.read("demos/TEST.dm_26", 64 * 1024 * 1024).unwrap().expect("TEST demo").bytes;
        let mut presenter = PlayerPresenter::new(assets, false).unwrap();
        presenter.set_async_loading(false);
        let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
        let mut decoder = Decoder::new();
        let mut game = ClientGameState::new();
        let mut npc_frames = 0;
        let mut npcs_with_body = HashSet::new();
        let mut npcs_with_blade = HashSet::new();
        let mut infos = Vec::new();
        while let Some(record) = reader.next_record().unwrap() {
            let packet = decoder.parse_packet(record.sequence, &record.payload).unwrap();
            for event in packet.events {
                match event {
                    Event::Gamestate { server_command_sequence, .. } => {
                        game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                    }
                    Event::ServerCommand(command) => game.queue_server_command(command),
                    Event::Snapshot { .. } => {
                        let snapshot = decoder.latest_snapshot().unwrap();
                        game.set_initial_snapshot(snapshot).unwrap();
                        let entities = game.present_entities(snapshot.server_time).unwrap();
                        let npcs = entities.iter().filter(|entity| entity.entity_type == ET_NPC).collect::<Vec<_>>();
                        if npcs.is_empty() { continue; }
                        npc_frames += 1;
                        let draws = presenter.present_snapshot_players(&entities, &game, &[], snapshot.server_time, None, None, None, None);
                        for npc in npcs {
                            if infos.len() < 3 {
                                let info = game.npc_client_info(&npc.state).unwrap();
                                infos.push(format!("{}/{} sabers={}+{} colors={}/{} def={} bolt={:?}",
                                    info.model_name, info.skin_name, info.saber_name, info.saber2_name,
                                    info.saber_color, info.saber2_color, info.definition_saber_colors,
                                    npc.state.field_i32("boltToPlayer")));
                            }
                            let own = draws.iter().filter(|surface| surface.entity_num == npc.number).count();
                            if own >= 10 { npcs_with_body.insert(npc.number); }
                            if presenter
                                .reported_saber_diagnostics
                                .contains(&format!("held:{}:0:0:submitted", npc.number))
                            {
                                npcs_with_blade.insert(npc.number);
                            }
                        }
                    }
                    _ => {}
                }
            }
            if npc_frames >= 400 { break; }
        }
        println!("NPC REGRESSION frames={npc_frames} body={} blade={} infos={infos:?}", npcs_with_body.len(), npcs_with_blade.len());
        assert!(npc_frames > 0);
        assert!(!npcs_with_body.is_empty(), "no reborn NPC submitted body geometry");
        assert!(!npcs_with_blade.is_empty(), "no reborn NPC submitted a saber blade");
    }

    /// Runs the actual demo -> clientinfo -> model/pose -> CPU geometry path.
    /// Requires user-owned assets; no window or GPU is needed.
    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock PK3s and demos/cheezyVsource.dm_26"]
    fn demo_missing_opponent_submits_fallback_geometry() {
        use jka_protocol::{demo::DemoReader, server::{Decoder, Event}};
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let bytes = assets.read("demos/cheezyVsource.dm_26", 64 * 1024 * 1024)
            .unwrap().expect("regression demo").bytes;
        let mut presenter = PlayerPresenter::new(assets, false).unwrap();
        presenter.set_async_loading(false);
        let mut reader = DemoReader::new(std::io::Cursor::new(bytes));
        let mut decoder = Decoder::new();
        let mut game = ClientGameState::new();
        let mut verified = false;
        while let Some(record) = reader.next_record().unwrap() {
            let packet = decoder.parse_packet(record.sequence, &record.payload).unwrap();
            for event in packet.events {
                match event {
                    Event::Gamestate { server_command_sequence, .. } => {
                        game.reset_gamestate(&decoder.configstrings, server_command_sequence);
                    }
                    Event::ServerCommand(command) => game.queue_server_command(command),
                    Event::Snapshot { .. } => {
                        let snapshot = decoder.latest_snapshot().unwrap();
                        game.set_initial_snapshot(snapshot).unwrap();
                        let entities = game.present_entities(snapshot.server_time).unwrap();
                        let Some(opponent) = entities.iter().find(|entity| entity.number == 2
                            && entity.entity_type == ET_PLAYER) else { continue; };
                        let client_num = opponent.state.field_i32("clientNum").unwrap() as usize;
                        let info = game.client_info(client_num, &[]).unwrap();
                        if presenter.try_load_model(&info).is_ok() { continue; }
                        println!("REGRESSION opponent candidate: entity=2 model={} duelIndex={:?} duelInProgress={:?}",
                            info.model_cvar(), snapshot.player_state.field_i32("duelIndex"),
                            snapshot.player_state.field_i32("duelInProgress"));
                        assert_eq!(snapshot.player_state.field_i32("duelIndex"), Some(2));
                        assert_eq!(snapshot.player_state.field_i32("duelInProgress"), Some(1));
                        let fallback = presenter.load_model(&info).unwrap();
                        assert!(fallback.key.starts_with("models/players/kyle/model.glm|"));
                        let again = presenter.load_model(&info).unwrap();
                        assert!(Arc::ptr_eq(&fallback, &again), "fallback assets must be reused");
                        let draws = presenter.present_snapshot_players(
                            &entities, &game, &[], snapshot.server_time, None, None, None, None,
                        );
                        assert!(draws.iter().any(|surface| surface.entity_num == 2
                            && !surface.vertices.is_empty() && !surface.indices.is_empty()));
                        assert!(presenter.player_diagnostics[&2].contains("bodySurfaces=19 submitted=1"));
                        // Configstring changes must select new assets, not a stale fallback alias.
                        let jan = crate::cgame::ClientInfo::solo_model("jan");
                        assert!(presenter.load_model(&jan).unwrap().key.starts_with("models/players/jan/"));
                        verified = true;
                        break;
                    }
                    _ => {}
                }
            }
            if verified { break; }
        }
        assert!(verified, "demo did not exercise missing entity-2 model fallback");
    }

    #[test]
    fn weapon_attachment_follows_cg_player_copy_rules() {
        let mut instance = None;
        let mut cent_weapon = 0;
        let mut attached = None;
        let mut saber1 = false;
        let mut saber2 = false;

        update_weapon_attachment(
            &mut instance, &mut cent_weapon, &mut attached, &mut saber1, &mut saber2,
            5, Some(5), false, false, false, true, false,
        );
        assert_eq!((instance, cent_weapon, attached, saber1, saber2), (Some(5), 5, Some(5), false, false));

        update_weapon_attachment(
            &mut instance, &mut cent_weapon, &mut attached, &mut saber1, &mut saber2,
            0, None, false, false, false, true, false,
        );
        assert_eq!(attached, Some(5), "a NULL G2 weapon instance does not remove the old model");
        assert_eq!(cent_weapon, 0);

        update_weapon_attachment(
            &mut instance, &mut cent_weapon, &mut attached, &mut saber1, &mut saber2,
            WP_MELEE, Some(WP_MELEE), false, false, false, true, false,
        );
        assert_eq!((attached, saber1, saber2), (None, false, false));

        update_weapon_attachment(
            &mut instance, &mut cent_weapon, &mut attached, &mut saber1, &mut saber2,
            11, Some(11), false, false, false, true, false,
        );
        update_weapon_attachment(
            &mut instance, &mut cent_weapon, &mut attached, &mut saber1, &mut saber2,
            WP_SABER, Some(WP_SABER), true, true, false, true, true,
        );
        assert_eq!(instance, None, "dead CG_Player clears only ghoul2weapon");
        assert_eq!(cent_weapon, 11, "dead CG_Player preserves cent->weapon");
        assert_eq!((attached, saber1, saber2), (Some(11), false, false));

        update_weapon_attachment(
            &mut instance, &mut cent_weapon, &mut attached, &mut saber1, &mut saber2,
            WP_SABER, Some(WP_SABER), false, false, false, true, true,
        );
        assert_eq!((cent_weapon, attached, saber1, saber2), (WP_SABER, None, true, true));
        assert!(weapon_world_model(8).unwrap().ends_with("heavy_repeater_w.glm"));
        assert!(weapon_world_model(0).is_none());
    }

    #[test]
    fn reset_player_entity_copies_both_dual_sabers_before_primary_flight_removal() {
        let mut instance = None;
        let mut cent_weapon = 0;
        let mut attached = None;
        let mut saber1 = false;
        let mut saber2 = false;

        // CG_SetInitialSnapshot -> CG_ResetEntity -> CG_ResetPlayerEntity.
        // CG_CopyG2WeaponInstance(WP_SABER) copies *both* configured saber
        // models before CG_Player sees that saber 0 is already in flight.
        reset_remote_player_saber_attachment(
            &mut instance,
            &mut cent_weapon,
            &mut attached,
            &mut saber1,
            &mut saber2,
            WP_SABER,
            true,
            true,
            true,
        );
        assert_eq!(instance, Some(WP_SABER));
        assert_eq!(cent_weapon, WP_SABER);
        assert_eq!((attached, saber1, saber2), (None, true, true));

        // The following CG_Player weapon update is pointer-equal and therefore
        // does not recopy anything merely because the primary is in flight.
        update_weapon_attachment(
            &mut instance,
            &mut cent_weapon,
            &mut attached,
            &mut saber1,
            &mut saber2,
            WP_SABER,
            Some(WP_SABER),
            true,
            false,
            false,
            true,
            true,
        );
        assert_eq!((saber1, saber2), (true, true));

        // cg_players.c's saberInFlight model path removes Ghoul2 model index 1
        // only. Model index 2 is deliberately untouched.
        saber1 = false;
        assert_eq!((saber1, saber2), (false, true));

        // On subsequent in-flight frames the index-1 g2HasWeapon test resets
        // tracking, but saberInFlight pins ghoul2weapon and still must not
        // disturb model index 2.
        update_weapon_attachment(
            &mut instance,
            &mut cent_weapon,
            &mut attached,
            &mut saber1,
            &mut saber2,
            WP_SABER,
            Some(WP_SABER),
            true,
            false,
            false,
            true,
            true,
        );
        assert_eq!((saber1, saber2), (false, true));
    }

    #[test]
    fn saber_return_rearms_missing_model_slot_like_openjk_g2hasweapon() {
        let mut instance = Some(WP_SABER);
        let mut cent_weapon = WP_SABER;
        let mut attached = None;
        let mut saber1 = false;
        let mut saber2 = false;

        // Model index 1 has been removed by the in-flight saber path. While the
        // saber is still away, OpenJK's g2HasWeapon check clears cent->weapon,
        // then saberInFlight pins ghoul2weapon to the saber instance so it is
        // not immediately recopied into the hand.
        update_weapon_attachment(
            &mut instance, &mut cent_weapon, &mut attached, &mut saber1, &mut saber2,
            WP_SABER, Some(WP_SABER), true, false, false, true, false,
        );
        assert_eq!(instance, Some(WP_SABER));
        assert_eq!(cent_weapon, 0);
        assert_eq!((attached, saber1, saber2), (None, false, false));

        // On the first frame where saberInFlight clears, model index 1 is still
        // absent. The same g2HasWeapon check therefore clears ghoul2weapon, the
        // normal mismatch test fires, and CG_CopyG2WeaponInstance semantics
        // restore the hilt. This is the catch/recovery transition from OpenJK.
        update_weapon_attachment(
            &mut instance, &mut cent_weapon, &mut attached, &mut saber1, &mut saber2,
            WP_SABER, Some(WP_SABER), false, false, false, true, false,
        );
        assert_eq!(instance, Some(WP_SABER));
        assert_eq!(cent_weapon, WP_SABER);
        assert_eq!((attached, saber1, saber2), (None, true, false));
    }

    #[test]
    fn angles_to_axis_matches_jka_cardinal_yaw() {
        let axis = angles_to_axis([0.0, 90.0, 0.0]);
        assert!(axis[0][0].abs() < 1.0e-5);
        assert!((axis[0][1] - 1.0).abs() < 1.0e-5);
        assert!((axis[2][2] - 1.0).abs() < 1.0e-5);
    }

    #[test]
    fn glm_winding_is_flipped_for_wgpu_backface_culling() {
        assert_eq!(
            glm_indices_for_wgpu(vec![0, 1, 2, 3, 4, 5]),
            vec![0, 2, 1, 3, 5, 4]
        );
    }

    #[test]
    fn player_entity_reset_matches_openjk_teleport_and_client_discontinuity() {
        assert!(!openjk_player_entity_needs_reset(0, 2, 0, 2, false, false));
        assert!(openjk_player_entity_needs_reset(0, 2, EF_TELEPORT_BIT, 2, false, false));
        assert!(openjk_player_entity_needs_reset(EF_TELEPORT_BIT, 2, 0, 2, false, false));
        assert!(openjk_player_entity_needs_reset(0, 2, 0, 3, false, false));
        assert!(openjk_player_entity_needs_reset(0, 2, 0, 2, true, false));
        assert!(openjk_player_entity_needs_reset(0, 2, 0, 2, false, true));
    }

    #[test]
    fn player_angle_reset_preserves_openjk_clientinfo_state() {
        let mut state = PlayerAngleState {
            torso_yawing: 1,
            torso_pitching: 1,
            legs_yawing: 1,
            torso_yaw_angle: 1.0,
            torso_pitch_angle: 2.0,
            legs_yaw_angle: 3.0,
            corr_time: 41,
            look_time: 42,
            super_smooth_time: 43,
            last_head_angles: [4.0, 5.0, 6.0],
        };
        state.reset_entity_swing_from_angles([10.0, 20.0, 30.0]);
        assert_eq!(state.torso_yawing, 0);
        assert_eq!(state.torso_pitching, 0);
        assert_eq!(state.legs_yawing, 0);
        assert_eq!(state.torso_yaw_angle, 20.0);
        assert_eq!(state.torso_pitch_angle, 10.0);
        assert_eq!(state.legs_yaw_angle, 20.0);
        assert_eq!(state.super_smooth_time, 0);
        assert_eq!(state.corr_time, 41);
        assert_eq!(state.look_time, 42);
        assert_eq!(state.last_head_angles, [4.0, 5.0, 6.0]);
    }
}

fn transform_jka_model_point(
    point: [f32; 3],
    axis: [[f32; 3]; 3],
    origin: [f32; 3],
) -> [f32; 3] {
    [
        origin[0] + axis[0][0] * point[0] + axis[1][0] * point[1] + axis[2][0] * point[2],
        origin[1] + axis[0][1] * point[0] + axis[1][1] * point[1] + axis[2][1] * point[2],
        origin[2] + axis[0][2] * point[0] + axis[1][2] * point[1] + axis[2][2] * point[2],
    ]
}

fn transform_jka_model_vector(vector: [f32; 3], axis: [[f32; 3]; 3]) -> [f32; 3] {
    [
        axis[0][0] * vector[0] + axis[1][0] * vector[1] + axis[2][0] * vector[2],
        axis[0][1] * vector[0] + axis[1][1] * vector[1] + axis[2][1] * vector[2],
        axis[0][2] * vector[0] + axis[1][2] * vector[1] + axis[2][2] * vector[2],
    ]
}

fn normalize_vec3(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length <= f32::EPSILON {
        [1.0, 0.0, 0.0]
    } else {
        [v[0] / length, v[1] / length, v[2] / length]
    }
}

/// CG_AddSaberBlade color selection: userinfo c1/c2 (or NPC boltToPlayer)
/// unless the NPC path uses the definition's authored per-blade color.
fn blade_color(
    info: &crate::cgame::ClientInfo,
    client_color: i32,
    blade: &jka_assets::saber::SaberBladeDefinition,
    saber_team_colors: Option<bool>,
) -> i32 {
    let color = if info.definition_saber_colors { blade.color } else { client_color };
    let color = saber_team_colors
        .map_or(color, |force| crate::cgame::team_saber_color(info, color, force));
    crate::cgame::apply_plugin_saber_color(color, info.plugin_disable)
}

/// animTable index of a named animation (cached).
fn animation_number(name: &str) -> Option<i32> {
    use std::sync::OnceLock;
    static TABLE: OnceLock<HashMap<&'static str, i32>> = OnceLock::new();
    TABLE
        .get_or_init(|| (0..2048).filter_map(|i| jka_movement::animation_name(i).map(|n| (n, i))).collect())
        .get(name)
        .copied()
}

/// CG_InitG2Weapons: g2WeaponInstances[giTag] = world_model[0] of the last
/// IT_WEAPON item with that tag (later bg_itemlist entries overwrite).
fn weapon_world_model(weapon: i32) -> Option<String> {
    use std::sync::OnceLock;
    static TABLE: OnceLock<HashMap<i32, String>> = OnceLock::new();
    TABLE
        .get_or_init(|| {
            let mut table = HashMap::new();
            for index in 1..jka_movement::bg_item_count() {
                if let Some(item) = jka_movement::bg_item(index) {
                    if item.item_type == 1 && !item.world_model.is_empty() {
                        table.insert(item.tag, item.world_model);
                    }
                }
            }
            table
        })
        .get(&weapon)
        .cloned()
}

/// RF_FORCE_ENT_ALPHA below 1 needs a blended pass for normally opaque stages.
pub(super) fn blend_for_alpha(mode: DynamicModelAlphaMode, alpha: f32) -> DynamicModelAlphaMode {
    if alpha >= 1.0 {
        return mode;
    }
    match mode {
        DynamicModelAlphaMode::Opaque => DynamicModelAlphaMode::Blend,
        DynamicModelAlphaMode::Mask => DynamicModelAlphaMode::MaskBlend,
        mode => mode,
    }
}

pub(super) fn saber_name_is_removed(name: &str) -> bool {
    name.eq_ignore_ascii_case("none") || name.eq_ignore_ascii_case("remove")
}

#[allow(clippy::too_many_arguments)]
fn saber_blade_fx_request(
    origin: [f32; 3],
    direction: [f32; 3],
    length: f32,
    length_max: f32,
    radius: f32,
    color: i32,
    entity_alpha: f32,
    entity_num: u16,
    saber_num: u8,
    blade_num: u8,
    saber_move: i32,
    torso_anim: i32,
    saber_in_flight: bool,
    trail_style: i32,
    num_blades: u8,
    no_dlight: bool,
    no_wall_marks: bool,
) -> Option<PlayerFxRequest> {
    // A zero/near-zero terminal sample clears WeaponFx contact history when
    // OpenJK retracts a dead blade. Geometry itself is still omitted downstream.
    if length < 0.0 || radius <= 0.0 {
        return None;
    }
    Some(PlayerFxRequest::SaberBlade {
        origin,
        direction: normalize_vec3(direction),
        length,
        length_max,
        radius,
        color,
        entity_alpha: entity_alpha.clamp(0.0, 1.0),
        entity_num,
        saber_num,
        blade_num,
        saber_move,
        torso_anim,
        saber_in_flight,
        trail_style,
        num_blades,
        no_dlight,
        no_wall_marks,
    })
}

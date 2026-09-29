//! Shared JKA player visual presentation for snapshot, followed, and local players.
//!
//! The data flow mirrors OpenJK's multiplayer cgame path: `ET_PLAYER` resolves
//! `clientInfo_t` from `CS_PLAYERS`, registers `model.glm` + skin, runs the
//! humanoid animation state, then submits Ghoul2 surfaces through the selected
//! CPU/worker/GPU skinning backend. Snapshot ownership stays in `cgame.rs` so a
//! network, demo, and local/offline sources use this exact same presentation code.

use crate::{
    asset_jobs::{self, AssetPriority, AssetRegistry, AssetSource, AssetState, Requested},
    cgame::{ClientGameState, ForcedPlayerModels, Ghoul2ServerCommand, PresentationEvent, PresentedEntity, ET_BODY, ET_NPC, ET_PLAYER, GT_SIEGE},
    materials::{self, TextureData, Textures},
    renderer::{
        DynamicModelAlphaMode, DynamicModelSurface, DynamicWireframeClass, DynamicModelVertex, Ghoul2GpuBone,
        Ghoul2GpuSkinning, Ghoul2GpuVertex,
    },
    scene,
    ui::Ghoul2SkinningMode,
};
use jka_assets::{
    animation::{animation_index, load_humanoid_animations, AnimationSet},
    ghoul2::{
        model_bolt_matrix, multiply_3x4, openjk_default_gla, parse_gla, parse_glm,
        skin_glm_surface, GlaAnimation, Ghoul2Animator, Ghoul2SkinnedSurface, GlmModel, GlmSurface, Matrix3x4,
        BONE_ANIM_OVERRIDE_FREEZE, BONE_ANIM_OVERRIDE_LOOP, OPENJK_DEFAULT_GLA_NAME,
    },
    pk3::AssetSearchPath,
    siege::find_siege_class_visual,
    saber::{
        load_saber_animation_scales, load_saber_definitions, SaberAnimationScales, SaberDefinition,
        SaberDefinitions,
    },
    shader::Shader,
    skin::load_skin,
    vehicle::{load_vehicle_definitions, VehicleDefinition, VehicleDefinitions},
};
use super::player_animation::PlayerAnimationState;
use super::ragdoll::{PhysicsMapMesh, RagdollConfig, RagdollWorld};
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
const EF_NODRAW: i32 = 1 << 8;
const EF_DEAD: i32 = 1 << 1;
const EF_RAG: i32 = 1 << 6;
const EF2_SHIP_DEATH: i32 = 1 << 7;

// OpenJK MP CG_PlayerShadow / bg_public.h / teams.h values. Blob shadows are
// deliberately a CGame presentation feature, not part of the CSM/RT paths.
const PW_CLOAKED: i32 = 11;
const CLASS_REMOTE: i32 = 39;
const CLASS_SEEKER: i32 = 41;
const BLOB_SHADOW_DISTANCE: f32 = 128.0;
const BLOB_SHADOW_RADIUS: f32 = 24.0;
const BLOB_SHADOW_DROID_RADIUS: f32 = 8.0;
const BLOB_SHADOW_MINS: [f32; 3] = [-15.0, -15.0, 0.0];
const BLOB_SHADOW_MAXS: [f32; 3] = [15.0, 15.0, 2.0];
const MASK_PLAYERSOLID: i32 = 0x0000_0001 | 0x0000_0010 | 0x0000_0100 | 0x0000_1000;
// OpenJK gets polygonOffset from the markShadow shader. Dynamic-model effects
// do not currently carry shader polygonOffset state, so lift the same decal a
// tiny amount along the hit normal to avoid z-fighting without a new pipeline.
const BLOB_SHADOW_SURFACE_LIFT: f32 = 0.125;

const MAX_GLM_BYTES: usize = 64 * 1024 * 1024;
const MAX_GLA_BYTES: usize = 128 * 1024 * 1024;
const MAX_SKIN_BYTES: usize = 4 * 1024 * 1024;


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
}

impl Ghoul2PresentationView {
    pub fn new(
        position: [f32; 3],
        forward: [f32; 3],
        fov_y: f32,
        aspect: f32,
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

    fn sphere_outside(self, jka_origin: [f32; 3], radius: f32) -> bool {
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

/// Faithful G2_ComputeLOD shape with stock OpenJK r_lodscale=5 and
/// r_autolodscalevalue=0. `global_lod_bias` is the archived JKA `r_lodbias`
/// cvar. Per-model mLodBias is not authored by our current GLM presentation
/// state, so it is zero; OpenJK therefore effectively uses max(r_lodbias, 0).
fn ghoul2_lod_for_view(
    view: Ghoul2PresentationView,
    entity: &PresentedEntity,
    render_origin: [f32; 3],
    num_lods: usize,
    global_lod_bias: i32,
) -> usize {
    if num_lods < 2 {
        return 0;
    }
    let scaled_radius = 0.75 * ghoul2_entity_radius(entity) * ghoul2_model_scale(entity);
    let projected_radius = view.projected_radius(render_origin, scaled_radius);
    let flod = if projected_radius != 0.0 {
        1.0 - projected_radius * 5.0
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
    key: Arc<str>,
    vertices: Arc<Vec<Ghoul2GpuVertex>>,
    indices: Arc<Vec<u32>>,
}

#[derive(Clone)]
struct PlayerSurfaceAsset {
    surface_index: usize,
    texture: Option<Arc<TextureData>>,
    alpha_mode: DynamicModelAlphaMode,
    /// Asset Viewer only: this surface had no usable authored material, so
    /// render it neutral gray instead of disappearing into the black preview.
    fallback_gray: bool,
    /// Static bind-pose mesh for each authored GLM LOD. A surface can be
    /// absent from a lower LOD, matching Ghoul2's per-LOD surface tables.
    gpu_meshes: Vec<Option<Arc<Ghoul2GpuMeshSource>>>,
}

struct PlayerModelAsset {
    key: String,
    glm: Arc<GlmModel>,
    gla: Arc<GlaAnimation>,
    surfaces: Vec<PlayerSurfaceAsset>,
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
}

/// Force-power visuals CG_Player hands to the FX system / local entities.
#[derive(Clone, Debug, PartialEq)]
pub enum PlayerFxRequest {
    /// FX_PlayEntityEffectID at a hand bolt (lightning, drain).
    Effect { name: &'static str, origin: [f32; 3], axis: [[f32; 3]; 3] },
    /// CG_DoSaber blade presentation. The FX layer owns the view-facing geometry
    /// so authored saber shaders go through the same material path as other FX.
    SaberBlade {
        origin: [f32; 3],
        direction: [f32; 3],
        length: f32,
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
}

const EF_BODYPUSH: i32 = 1 << 19;
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

fn build_gpu_mesh_source(
    model_qpath: &str,
    lod_index: usize,
    surface: &GlmSurface,
) -> Result<Arc<Ghoul2GpuMeshSource>, String> {
    if surface.vertices.len() != surface.texcoords.len() {
        return Err(format!(
            "GLM surface {} has {} vertices but {} texcoords",
            surface.surface_index, surface.vertices.len(), surface.texcoords.len()
        ));
    }
    let mut vertices = Vec::with_capacity(surface.vertices.len());
    for (vertex_index, (vertex, &uv)) in surface.vertices.iter().zip(&surface.texcoords).enumerate() {
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
        vertices.push(Ghoul2GpuVertex {
            position: vertex.position,
            normal: vertex.normal,
            uv,
            bone_indices,
            weights,
            weight_count: vertex.weights.len() as u32,
            _padding: [0; 3],
        });
    }
    let mut indices = Vec::with_capacity(surface.triangles.len() * 3);
    for (triangle_index, triangle) in surface.triangles.iter().enumerate() {
        if triangle
            .iter()
            .any(|&index| index as usize >= surface.vertices.len())
        {
            return Err(format!(
                "GLM surface {} triangle {} has out-of-range vertex",
                surface.surface_index, triangle_index
            ));
        }
        let mut triangle = *triangle;
        triangle.swap(1, 2);
        indices.extend_from_slice(&triangle);
    }
    Ok(Arc::new(Ghoul2GpuMeshSource {
        key: Arc::<str>::from(format!(
            "{}#lod{}#surface{}",
            model_qpath.replace('\\', "/").to_ascii_lowercase(),
            lod_index,
            surface.surface_index
        )),
        vertices: Arc::new(vertices),
        indices: Arc::new(indices),
    }))
}


fn build_lod_gpu_meshes(
    model_qpath: &str,
    glm: &GlmModel,
    surface_index: usize,
) -> Result<Vec<Option<Arc<Ghoul2GpuMeshSource>>>, String> {
    glm.lods
        .iter()
        .enumerate()
        .map(|(lod_index, lod)| {
            lod.surfaces
                .iter()
                .find(|surface| surface.surface_index == surface_index)
                .map(|surface| build_gpu_mesh_source(model_qpath, lod_index, surface))
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
    fn load_gla(&mut self, qpath: &str) -> Result<Arc<GlaAnimation>, String>;
    fn load_skin(&mut self, qpath: &str) -> Result<Vec<jka_assets::skin::SkinSurface>, String>;
    fn resolve_material(&mut self, shader_name: &str) -> (Option<Arc<TextureData>>, DynamicModelAlphaMode);
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

    fn load_gla(&mut self, qpath: &str) -> Result<Arc<GlaAnimation>, String> {
        self.presenter.load_glm_animation(qpath)
    }

    fn load_skin(&mut self, qpath: &str) -> Result<Vec<jka_assets::skin::SkinSurface>, String> {
        self.presenter.load_skin_qpath(qpath)
    }

    fn resolve_material(&mut self, shader_name: &str) -> (Option<Arc<TextureData>>, DynamicModelAlphaMode) {
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

    fn resolve_material(&mut self, shader_name: &str) -> (Option<Arc<TextureData>>, DynamicModelAlphaMode) {
        let (image_name, clamp, alpha_mode) = material_stage(&self.shared.shaders, shader_name);
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
                        println!("PLAYER TEXTURE WARNING: {warning}");
                    }
                }
                texture
            })
            .clone();
        (texture, alpha_mode)
    }
}

/// The primary stage of a JKA shader decides the image, clamping and alpha
/// mode of a player/Ghoul2 surface.
fn material_stage<'a>(
    shaders: &'a BTreeMap<String, Shader>,
    shader_name: &'a str,
) -> (&'a str, bool, DynamicModelAlphaMode) {
    let shader_key = shader_name.replace('\\', "/").to_ascii_lowercase();
    shaders
        .get(&shader_key)
        .and_then(Shader::primary)
        .map(|stage| {
            let alpha_mode = if !stage.alpha_test.trim().is_empty() {
                DynamicModelAlphaMode::Mask
            } else {
                crate::fx::draw::FxBlend::from_blend_func(&stage.blend)
                    .custom_shader_alpha_mode()
            };
            (stage.image.as_str(), stage.clamp, alpha_mode)
        })
        .unwrap_or((shader_name, false, DynamicModelAlphaMode::Opaque))
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

    let skin_qpath = info.skin_qpath();
    let skin = match source.load_skin(&skin_qpath) {
        Ok(skin) => skin,
        Err(primary_error) => {
            let fallback = info.default_skin_qpath();
            println!(
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
        let shader_name = skin_map
            .get(&hierarchy.name.to_ascii_lowercase())
            .map(String::as_str)
            .unwrap_or(hierarchy.shader.as_str());
        if shader_name.is_empty() || shader_name.eq_ignore_ascii_case("*off") {
            continue;
        }
        let (texture, alpha_mode) = source.resolve_material(shader_name);
        surfaces.push(PlayerSurfaceAsset {
            surface_index: surface.surface_index,
            texture,
            alpha_mode,
            fallback_gray: false,
            gpu_meshes: build_lod_gpu_meshes(&model_qpath, &glm, surface.surface_index)?,
        });
    }

    println!(
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
                println!(
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
        let (texture, alpha_mode) = source.resolve_material(shader_name);
        let fallback_gray = preview_fallback
            && (missing_shader
                || (!shader_name.eq_ignore_ascii_case("$whiteimage") && texture.is_none()));
        surfaces.push(PlayerSurfaceAsset {
            surface_index: surface.surface_index,
            texture,
            alpha_mode,
            fallback_gray,
            gpu_meshes: build_lod_gpu_meshes(&model_qpath, &glm, surface.surface_index)?,
        });
    }
    println!(
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

/// Asset/runtime state corresponding to the player-model subset of OpenJK cgame.
/// Models and textures are cached by qpath; animation state is kept per entity,
/// matching the persistent `centity_t` ownership in the original client.
pub struct PlayerPresenter {
    /// This frame's force-power FX requests (drained by the caller).
    fx_requests: Vec<PlayerFxRequest>,
    perf: Ghoul2PerfStats,
    /// Viewer (cg.snap->ps) client and duel state for shells like the duel bubble.
    viewer_client: i32,
    viewer_dueling: bool,
    assets: AssetSearchPath,
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
    /// Broadsword-compatible presentation seam backed by Rapier.
    ragdolls: RagdollWorld,
    /// Vanilla entityState does not transmit forceGripEntityNum. Preserve the
    /// last target per active gripper and reacquire with OpenJK's 256-unit
    /// view trace when needed. The local victim uses the exact playerState
    /// forceGripCripple flag supplied by App.
    force_grip_targets: HashMap<u16, u16>,
    force_gripped_entities: HashSet<u16>,
    thrown_sabers: HashMap<u16, SaberThrowState>,
    /// OpenJK CG_BodyQueueCopy state delivered by reliable `ircg`.
    body_queue_copies: HashMap<u16, BodyQueueCopyState>,
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
    blob_shadow_vertices: Vec<DynamicModelVertex>,
    blob_shadow_indices: Vec<u32>,
    blob_shadow_texture: Option<Arc<TextureData>>,
    blob_shadow_alpha_mode: DynamicModelAlphaMode,
    blob_shadow_texture_resolved: bool,
    blob_shadow_asset_warned: bool,
    lod_bias: i32,
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
        self.entities.clear();
        self.force_grip_targets.clear();
        self.force_gripped_entities.clear();
        self.thrown_sabers.clear();
        self.body_queue_copies.clear();
        self.saber_blade_lengths.clear();
        self.blob_shadow_vertices.clear();
        self.blob_shadow_indices.clear();
        self.ragdolls.reset_dynamic_for_seek();
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
        let saber_scales = load_saber_animation_scales(&mut assets)?;
        let saber_definitions = load_saber_definitions(&mut assets)?;
        let vehicle_definitions = load_vehicle_definitions(&mut assets)?;
        let mut shader_warnings = Vec::new();
        let (shaders, diagnostics) =
            materials::shader_library(&mut assets, &mut shader_warnings, pbr)?;
        println!(
            "PLAYER ASSETS: humanoid animations={} saberDefs={} vehicleDefs={} shaderDefs={} mtrDefs={}",
            animations.animations.len(),
            saber_definitions.len(),
            vehicle_definitions.len(),
            diagnostics.shader_definitions,
            diagnostics.mtr_definitions,
        );
        for warning in shader_warnings {
            println!("PLAYER MATERIAL WARNING: {warning}");
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
        println!("Ghoul2 skinning worker pool: {worker_count} thread(s)");
        let mut presenter = Self {
            assets,
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
            perf: Ghoul2PerfStats::default(),
            viewer_client: -1,
            viewer_dueling: false,
            entities: HashMap::new(),
            ragdolls: RagdollWorld::new(),
            force_grip_targets: HashMap::new(),
            force_gripped_entities: HashSet::new(),
            thrown_sabers: HashMap::new(),
            body_queue_copies: HashMap::new(),
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
            blob_shadow_vertices: Vec::new(),
            blob_shadow_indices: Vec::new(),
            blob_shadow_texture: None,
            blob_shadow_alpha_mode: DynamicModelAlphaMode::BlendUnlit,
            blob_shadow_texture_resolved: false,
            blob_shadow_asset_warned: false,
            lod_bias: 0,
            skinning_pool,
        };
        presenter.prime_default_player_models();
        Ok(presenter)
    }

    /// Force-power effect requests produced since the last call.
    pub fn drain_fx_requests(&mut self) -> Vec<PlayerFxRequest> {
        std::mem::take(&mut self.fx_requests)
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
            }
        }
    }

    /// OpenJK EV_DESTROY_WEAPON_MODEL removes Ghoul2 model index 1 only.
    pub fn apply_entity_event(&mut self, event: &PresentationEvent) {
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

    pub fn set_ragdoll_config(&mut self, config: RagdollConfig) {
        self.ragdolls.set_config(config);
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
            self.ragdolls.begin_force_grip_recovery(*entity, current_time);
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

    pub fn set_rt_shadow_casters_enabled(&mut self, enabled: bool) {
        self.rt_shadow_casters_enabled = enabled;
    }

    /// Install the current map collision world for OpenJK-style blob traces.
    /// This is map-scoped and must not be rebound every frame.
    pub fn set_collision_world(&mut self, world: Option<CollisionWorld>) {
        self.collision_world = world;
        self.blob_shadow_vertices.clear();
        self.blob_shadow_indices.clear();
    }

    /// Begin one CG_AddEntities blob-shadow collection pass. All eligible
    /// players/NPCs are packed into one dynamic surface so the OpenJK visual
    /// costs one draw rather than one temporary-poly draw per character.
    pub fn begin_blob_shadow_frame(&mut self, enabled: bool) {
        self.blob_shadows_enabled = enabled;
        self.blob_shadow_vertices.clear();
        self.blob_shadow_indices.clear();
    }

    /// Finish cg_shadows 1. Keep the stock `markShadow` image *and its authored
    /// first-stage blend mode*, while batching all blobs into one surface.
    pub fn finish_blob_shadow_frame(&mut self) -> Option<DynamicModelSurface> {
        if !self.blob_shadows_enabled || self.blob_shadow_indices.is_empty() {
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
            self.blob_shadow_vertices.clear();
            self.blob_shadow_indices.clear();
            return None;
        };
        Some(DynamicModelSurface {
            entity_num: u16::MAX,
            wireframe_class: DynamicWireframeClass::Effect,
            raster_visible: true,
            vertices: Arc::new(std::mem::take(&mut self.blob_shadow_vertices)),
            indices: Arc::new(std::mem::take(&mut self.blob_shadow_indices)),
            lighting_origin: None,
            rt_rigid: None,
            rt_skinned_key: None,
            ghoul2_gpu: None,
            fx_gpu_sprites: None,
            texture: Some(texture),
            // OpenJK registers `markShadow` as a shader and lets the renderer
            // honor the shader's authored blendFunc; preserve that here.
            alpha_mode: self.blob_shadow_alpha_mode,
        })
    }

    pub fn set_lod_bias(&mut self, lod_bias: i32) {
        // OpenJK's renderer cvar is not range-checked, but its Ghoul2 path
        // takes max(r_lodbias, modelBias). Our current per-model bias is 0,
        // so negative values have no effect; keep the stored runtime value in
        // the useful non-negative range.
        self.lod_bias = lod_bias.max(0);
    }

    #[allow(clippy::too_many_arguments)]
    fn render_glm_surfaces(
        &mut self,
        entity_num: u16,
        model_label: &str,
        glm: &GlmModel,
        surface_assets: &[PlayerSurfaceAsset],
        pose: &[Matrix3x4],
        lod_index: usize,
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        rgba: [f32; 4],
        custom_material: Option<(Option<Arc<TextureData>>, DynamicModelAlphaMode)>,
        apply_alpha_blend: bool,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let lod_index = lod_index.min(glm.lods.len().saturating_sub(1));
        let lod = glm
            .lods
            .get(lod_index)
            .ok_or_else(|| format!("{model_label} has no GLM LODs"))?;
        let jobs = surface_assets
            .iter()
            .filter_map(|asset| {
                let surface = lod
                    .surfaces
                    .iter()
                    .find(|surface| surface.surface_index == asset.surface_index)?;
                let gpu_mesh = asset.gpu_meshes.get(lod_index)?.as_ref()?;
                Some((asset, surface, gpu_mesh))
            })
            .collect::<Vec<_>>();

        if self.skinning_mode == Ghoul2SkinningMode::Gpu {
            let bones = gpu_bones_from_pose(pose);
            let empty_vertices = Arc::new(Vec::new());
            let mut draws = Vec::with_capacity(jobs.len());
            for (asset, _, gpu_mesh) in jobs {
                if gpu_mesh.vertices.is_empty() || gpu_mesh.indices.is_empty() {
                    continue;
                }
                self.perf.surfaces_skinned = self.perf.surfaces_skinned.saturating_add(1);
                self.perf.vertices_skinned = self
                    .perf
                    .vertices_skinned
                    .saturating_add(gpu_mesh.vertices.len() as u64);
                let (texture, base_alpha_mode) = custom_material
                    .clone()
                    .unwrap_or_else(|| (asset.texture.clone(), asset.alpha_mode));
                let surface_rgba = if custom_material.is_none() && asset.fallback_gray {
                    [0.58, 0.58, 0.58, rgba[3]]
                } else {
                    rgba
                };
                let alpha_mode = if apply_alpha_blend {
                    blend_for_alpha(base_alpha_mode, surface_rgba[3])
                } else {
                    base_alpha_mode
                };
                draws.push(DynamicModelSurface {
                    entity_num,
                    wireframe_class: DynamicWireframeClass::Player,
                    raster_visible: true,
                    vertices: Arc::clone(&empty_vertices),
                    indices: Arc::clone(&gpu_mesh.indices),
                    lighting_origin: Some(origin),
                    rt_rigid: None,
                    rt_skinned_key: self.rt_shadow_casters_enabled.then(|| Arc::clone(&gpu_mesh.key)),
                    ghoul2_gpu: Some(Ghoul2GpuSkinning {
                        mesh_key: Arc::clone(&gpu_mesh.key),
                        vertices: Arc::clone(&gpu_mesh.vertices),
                        indices: Arc::clone(&gpu_mesh.indices),
                        bones: Arc::clone(&bones),
                        axis,
                        origin,
                        color: surface_rgba,
                    }),
                    fx_gpu_sprites: None,
                    texture,
                    alpha_mode,
                });
            }
            return Ok(draws);
        }

        let used_workers = self.skinning_mode == Ghoul2SkinningMode::CpuWorkers && jobs.len() > 1;
        let skinned = if used_workers {
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

        let mut draws = Vec::with_capacity(jobs.len());
        for ((asset, _, gpu_mesh), skinned) in jobs.into_iter().zip(skinned) {
            let skinned = skinned?;
            let surface_rgba = if custom_material.is_none() && asset.fallback_gray {
                [0.58, 0.58, 0.58, rgba[3]]
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
            let vertices = skinned
                .vertices
                .into_iter()
                .map(|vertex| DynamicModelVertex {
                    position: transform_model_point(vertex.position, axis, origin),
                    normal: transform_model_normal(vertex.normal, axis),
                    uv: vertex.uv,
                    color: surface_rgba,
                })
                .collect::<Vec<_>>();
            if vertices.is_empty() || gpu_mesh.indices.is_empty() {
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
            draws.push(DynamicModelSurface {
                entity_num,
                wireframe_class: DynamicWireframeClass::Player,
                raster_visible: true,
                vertices: Arc::new(vertices),
                indices: Arc::clone(&gpu_mesh.indices),
                lighting_origin: Some(origin),
                rt_rigid: None,
                rt_skinned_key: self.rt_shadow_casters_enabled.then(|| Arc::clone(&gpu_mesh.key)),
                ghoul2_gpu: None,
                fx_gpu_sprites: None,
                texture,
                alpha_mode,
            });
        }
        Ok(draws)
    }

    fn clone_surface_with_material(
        surface: &DynamicModelSurface,
        texture: Option<Arc<TextureData>>,
        alpha_mode: DynamicModelAlphaMode,
        color: [f32; 4],
    ) -> DynamicModelSurface {
        let ghoul2_gpu = surface.ghoul2_gpu.as_ref().map(|skin| Ghoul2GpuSkinning {
            mesh_key: Arc::clone(&skin.mesh_key),
            vertices: Arc::clone(&skin.vertices),
            indices: Arc::clone(&skin.indices),
            bones: Arc::clone(&skin.bones),
            axis: skin.axis,
            origin: skin.origin,
            color,
        });
        let vertices = if ghoul2_gpu.is_some() {
            Arc::clone(&surface.vertices)
        } else {
            Arc::new(
                surface
                    .vertices
                    .iter()
                    .map(|vertex| DynamicModelVertex { color, ..*vertex })
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
        let mut draws = self.render_glm_surfaces(
            entity.number,
            &requested,
            &model.glm,
            &model.surfaces,
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
        if let Some(snapshot) = game.current_snapshot() {
            self.viewer_client = snapshot.player_state.field_i32("clientNum").unwrap_or(-1);
            self.viewer_dueling = snapshot.player_state.field_i32("duelInProgress").unwrap_or(0) != 0;
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
        if let Some(entity_num) = preserve_entity {
            // The local/followed player is presented separately just like
            // OpenJK's predictedPlayerEntity, but it still owns persistent
            // centity/playerEntity state across frames.
            live_entities.insert(entity_num);
        }
        // CG_Player resolves lookTarget against cg_entities before BG_G2PlayerAngles.
        let entity_origins = entities
            .iter()
            .map(|entity| (entity.number, entity.origin))
            .collect::<HashMap<_, _>>();

        // ET_PLAYER uses CG_Player and ET_NPC uses CG_G2Animated. ET_BODY is
        // CG_General in OpenJK; this Rust presenter still owns the shared body
        // mesh/ragdoll submission, but its weapon state comes only from ircg.
        // NPCs resolve `ci = cent->npcClient` instead of cgs.clientinfo[].
        for entity in entities
            .iter()
            .filter(|entity| {
                entity.entity_type == ET_PLAYER
                    || entity.entity_type == ET_NPC
                    || entity.entity_type == ET_BODY
            })
        {
            live_entities.insert(entity.number);
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
            match self.present_player(
                entity,
                &info,
                current_time,
                1.0,
                look_target_origin,
                false,
                hidden_first_person_entity != Some(entity.number),
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

        // The followed/local player may not exist in packet entities but can
        // still be an exact forceGripCripple victim. Keep that live ragdoll
        // until present_player_entity runs later in the frame.
        live_ragdolls.extend(self.force_gripped_entities.iter().copied());
        self.entities.retain(|entity_num, _| live_entities.contains(entity_num));
        self.ragdolls.retain_visible(&live_ragdolls);
        self.thrown_sabers.retain(|entity_num, _| live_entities.contains(entity_num));
        self.player_diagnostics.retain(|entity_num, _| live_entities.contains(entity_num));
        if !self.logged_first_draw && !draws.is_empty() {
            println!(
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
    ) -> Result<Vec<DynamicModelSurface>, String> {
        if entity.entity_type != ET_BODY
            && entity.state.field_i32("eFlags").unwrap_or(0) & EF_NODRAW == 0
            && entity.state.field_i32("eFlags2").unwrap_or(0) & EF2_SHIP_DEATH == 0
        {
            self.queue_blob_shadow(entity);
        }
        self.present_player(
            entity,
            info,
            current_time,
            entity_alpha,
            look_target_origin,
            force_reset,
            submit_geometry,
            None,
        )
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
        view: Option<Ghoul2PresentationView>,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        self.poll_asset_completions();
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
        let has_primary_saber = !saber_name_is_removed(&info.saber_name);
        let has_secondary_saber = !info.saber2_name.is_empty()
            && !saber_name_is_removed(&info.saber2_name);
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
        let attached_weapon = runtime.attached_weapon;
        let attached_sabers = [runtime.primary_saber_attached, runtime.secondary_saber_attached];

        runtime.animation.update(
            &entity.state,
            &self.animations,
            &model.gla,
            info,
            &self.saber_scales,
            current_time,
            true,
        )?;

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
        let ragdoll_requested = corpse_ragdoll_requested || force_grip_ragdoll_requested;
        // OpenJK's corpse/Broadsword path forces yaw-only orientation. A living
        // Force-grip handoff must instead sample the exact normal player pose on
        // its first frame, then Rapier freezes that skinning pose internally.
        let rapier_corpse = corpse_ragdoll_requested && self.ragdolls.active();
        let (axis, torso_angles) = if rapier_corpse {
            runtime.last_angle_time = current_time;
            (angles_to_axis([0.0, entity.angles[1], 0.0]), [0.0; 3])
        } else {
            // OpenJK CG_Player calls CG_PlayerAnimation first, then
            // BG_G2PlayerAngles. BG_G2ClientSpineAngles may query the animated
            // Motion bolt to remove animation-authored torso motion before
            // distributing view angles.
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
                force_grip_ragdoll_requested,
                current_time,
            )?;
        } else {
            self.ragdolls.apply_force_grip_recovery(
                entity.number,
                &model.key,
                &mut pose,
                current_time,
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
        let mut draws = self.render_glm_surfaces(
            entity.number,
            &model.key,
            &model.glm,
            &model.surfaces,
            &pose,
            body_lod,
            render_axis,
            render_origin,
            [1.0, 1.0, 1.0, entity_alpha],
            None,
            true,
        )?;

        self.report_player_status(entity.number, format!(
            "clientNum={} requested={}/{} resolved={} bodySurfaces={} submitted={}",
            info.client_num, info.model_name, info.skin_name, model.key,
            draws.len(), usize::from(!draws.is_empty()),
        ));
        let shells = self.force_shells(entity, current_time);
        if !shells.is_empty() {
            let body = draws.clone();
            for (shader, rgba) in shells {
                let (texture, alpha_mode) = self.custom_shader_material(shader);
                for surface in &body {
                    draws.push(Self::clone_surface_with_material(
                        surface,
                        texture.clone(),
                        alpha_mode,
                        rgba,
                    ));
                }
            }
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
            &weapon_model.surfaces,
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
            println!("PLAYER PRESENTATION: entity={entity_num} {status}");
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

    /// Discard remembered asset failures and let worker VFS views re-mount
    /// packages. Call when content may have appeared (download finished,
    /// `fs_game` change), never per frame.
    #[allow(dead_code)]
    pub fn retry_failed_assets(&mut self) {
        let retried = self.async_models.retry_failed() + self.async_static_models.retry_failed();
        self.failed_models.clear();
        self.failed_saber_models.clear();
        self.asset_source = self.asset_source.refreshed();
        // Worker-side GLA/texture caches remember failures too.
        self.load_shared = Arc::new(ModelLoadShared::new(Arc::clone(&self.shaders)));
        if retried > 0 {
            println!("[ASSET] retrying {retried} previously failed asset(s)");
        }
    }

    fn register_client_saber_assets(
        &mut self,
        info: &crate::cgame::ClientInfo,
        entity_num: u16,
    ) {
        for (saber_num, saber_name) in [info.saber_name.as_str(), info.saber2_name.as_str()]
            .into_iter()
            .enumerate()
        {
            if saber_name.is_empty() {
                continue;
            }
            // OpenJK WP_SetSaber removes saber slot 1 for "none"/"remove" and
            // leaves an empty model. Do not feed those sentinels through the
            // missing-definition fallback, which would incorrectly create the
            // stock Reborn hilt.
            if saber_num == 1 && saber_name_is_removed(saber_name) {
                continue;
            }
            let definition = self.saber_definitions.definition_or_default(saber_name);
            if let Err(error) = self.load_saber_model_in_game(&definition) {
                self.report_saber_warning_once(entity_num, &error);
            }
        }
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
        if self.reported_saber_diagnostics.insert(key) {
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
        // guesses from currentState.
        if attached_sabers[0] && !saber_name_is_removed(&info.saber_name) {
            sabers.push((0usize, info.saber_name.as_str(), info.saber_color, "*r_hand"));
        }
        if attached_sabers[1]
            && !info.saber2_name.is_empty()
            && !saber_name_is_removed(&info.saber2_name)
        {
            sabers.push((1usize, info.saber2_name.as_str(), info.saber2_color, "*l_hand"));
        }

        for (saber_num, saber_name, saber_color, hand_name) in sabers {
            let definition = self.saber_definitions.definition_or_default(saber_name);
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
            let mut hilt_draws = self.render_glm_surfaces(
                entity.number,
                &definition.model,
                &hilt.glm,
                &hilt.surfaces,
                &hilt_pose,
                0,
                player_axis,
                player_origin,
                [1.0, 1.0, 1.0, entity_alpha],
                None,
                false,
            )?;
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
                let saber_color = blade_color(info, saber_color, &blade);
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
        if owner.state.field_i32("weapon").unwrap_or(0) != WP_SABER
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

        let mut definition = self.saber_definitions.definition_or_default(&info.saber_name);
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
            &hilt.surfaces,
            &hilt_pose,
            0,
            saber_axis,
            saber_entity.origin,
            [1.0, 1.0, 1.0, entity_alpha],
            None,
            false,
        )?;
        draws.append(&mut hilt_draws);

        let holstered = owner.state.field_i32("saberHolstered").unwrap_or(0);
        let blade_limit = if holstered == 1 && definition.num_blades > 1 {
            1
        } else {
            definition.num_blades
        };
        for blade_index in 0..blade_limit.min(definition.num_blades) {
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
            let saber_color = blade_color(info, info.saber_color, &blade);
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
            println!(
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
            let (name, hand) = if pass > FORCE_LEVEL_3 {
                let level = pass - FORCE_LEVEL_3;
                (if level > FORCE_LEVEL_2 { "mp/drainwide" } else { "mp/drain" }, "*l_hand")
            } else {
                // BOTH_FORCE_2HANDEDLIGHTNING_HOLD alternates hands (Q_irand).
                let two_handed = animation_number("BOTH_FORCE_2HANDEDLIGHTNING_HOLD")
                    .is_some_and(|anim| state.field_i32("torsoAnim").unwrap_or(-1) & !0x800 == anim);
                let hand = if two_handed && (current_time / 50) & 1 == 1 { "*r_hand" } else { "*l_hand" };
                (if pass > FORCE_LEVEL_2 { "force/lightningwide" } else { "force/lightning" }, hand)
            };
            if let Some(origin) = model_bolt_origin_timed(
                &mut self.perf, model, pose, axis, render_origin, hand,
            )? {
                self.fx_requests.push(PlayerFxRequest::Effect { name, origin, axis: fx_axis });
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
    fn force_shells(&self, entity: &PresentedEntity, current_time: i32) -> Vec<(&'static str, [f32; 4])> {
        let state = &entity.state;
        let active = state.field_i32("forcePowersActive").unwrap_or(0);
        let mut shells = Vec::new();
        if active & (1 << FP_RAGE) != 0 {
            // rand() & 1 each frame between the two electric shaders.
            let shader = if (current_time / 50) & 1 == 0 { "gfx/misc/electric" } else { "gfx/misc/fullbodyelectric2" };
            shells.push((shader, [1.0, 0.0, 0.0, 1.0]));
        }
        let number = i32::from(entity.number);
        if number != self.viewer_client
            && !self.viewer_dueling
            && state.field_i32("bolt1").unwrap_or(0) == 1
            && entity.entity_type == ET_PLAYER
        {
            // Duelists seen from outside the duel.
            shells.push(("gfx/misc/sightbubble", [100.0 / 255.0, 100.0 / 255.0, 1.0, 1.0]));
        }
        if active & (1 << FP_PROTECT) != 0 {
            shells.push(("gfx/misc/forceprotect", [0.0, 128.0 / 255.0, 0.0, 254.0 / 255.0]));
        }
        if state.field_i32("isJediMaster").unwrap_or(0) != 0 && number != self.viewer_client {
            shells.push(("powerups/forceshell", [100.0 / 255.0, 100.0 / 255.0, 1.0, 1.0]));
        }
        shells
    }

    /// OpenJK MP CG_PlayerShadow for cg_shadows 1. The original submits a
    /// temporary mark via CG_ImpactMark/R_MarkFragments; we preserve the trace,
    /// stock markShadow image, radius, yaw, fade and eligibility rules, then use
    /// one plane-aligned quad at the hit plane. This avoids CPU BSP mark-fragment
    /// clipping and batches every blob into one draw while retaining the same
    /// normal floor appearance.
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
        self.push_blob_shadow_quad(trace.end, trace.normal, yaw, radius, shade);
    }

    fn push_blob_shadow_quad(
        &mut self,
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
        let tangent = tangent0 * cos_yaw + bitangent0 * sin_yaw;
        let bitangent = -tangent0 * sin_yaw + bitangent0 * cos_yaw;
        let center = Vec3::from_array(origin) + normal * BLOB_SHADOW_SURFACE_LIFT;
        let tangent = tangent * radius;
        let bitangent = bitangent * radius;
        let corners = [
            center - tangent - bitangent,
            center + tangent - bitangent,
            center + tangent + bitangent,
            center - tangent + bitangent,
        ];
        let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        let base = match u32::try_from(self.blob_shadow_vertices.len()) {
            Ok(base) => base,
            Err(_) => return,
        };
        let render_normal = scene::render_position(normal.to_array());
        for (corner, uv) in corners.into_iter().zip(uvs) {
            self.blob_shadow_vertices.push(DynamicModelVertex {
                position: scene::render_position(corner.to_array()),
                normal: render_normal,
                uv,
                // CG_PlayerShadow passes (alpha, alpha, alpha, 1) into
                // CG_ImpactMark. Preserve that vertex color and let the
                // authored markShadow shader supply its own blendFunc.
                color: [shade, shade, shade, 1.0],
            });
        }
        self.blob_shadow_indices.extend_from_slice(&[
            base, base + 1, base + 2, base, base + 2, base + 3,
        ]);
    }

    /// A refEntity customShader: its first stage's texture with FX blending
    /// semantics (these shaders are unlit, vertex/entity colored).
    fn custom_shader_material(&mut self, shader_name: &str) -> (Option<Arc<TextureData>>, DynamicModelAlphaMode) {
        let key = shader_name.replace('\\', "/").to_ascii_lowercase();
        let stage = self.shaders.get(&key).and_then(Shader::primary).cloned();
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
        let model = self.load_static_glm(model_qpath, None, model_qpath)?;
        let pose_started = Instant::now();
        let pose = Ghoul2Animator::new(&model.gla).evaluate_pose_openjk_root(&model.gla, current_time)?;
        record_pose_eval(&mut self.perf, pose_started);
        let custom = custom_shader.map(|shader| self.custom_shader_material(shader));
        let draws = self.render_glm_surfaces(
            entity_num,
            model_qpath,
            &model.glm,
            &model.surfaces,
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
            &model.surfaces,
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
        let mut draws = self.render_glm_surfaces(
            0,
            &info.model_qpath(),
            &model.glm,
            &model.surfaces,
            &pose,
            0,
            axis,
            origin,
            [1.0; 4],
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
                    &hilt.surfaces,
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
                    let color = blade_color(info, client_color, &blade);
                    let secondary_style = definition.blade_style2_start > 0
                        && blade_index >= definition.blade_style2_start;
                    let trail_style = if secondary_style { definition.trail_style2 } else { definition.trail_style };
                    let no_wall_marks = if secondary_style { definition.no_wall_marks2 } else { definition.no_wall_marks };
                    if let Some(request) = saber_blade_fx_request(
                        origin_world,
                        dir_world,
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

        // Profile is intentionally studio-lit rather than map-lightgrid-lit.
        // This bakes a key + soft fill from vertex normals into preview-only
        // shaderRGBA. Gameplay models continue through the configured lighting.
        for surface in &mut draws {
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
            // preview; the studio lighting above should be stable on every map.
            surface.lighting_origin = None;
        }
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
            &model.surfaces,
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
            &model.surfaces,
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

    fn resolve_surface_material(
        &mut self,
        shader_name: &str,
    ) -> (Option<Arc<TextureData>>, DynamicModelAlphaMode) {
        let (image_name, clamp, alpha_mode) = material_stage(&self.shaders, shader_name);

        if image_name.eq_ignore_ascii_case("$whiteimage") {
            return (None, alpha_mode);
        }
        let texture = self
            .textures
            .load(&mut self.assets, image_name, clamp)
            .map(|index| {
                if !self.texture_arcs.contains_key(&index) {
                    let image = Arc::new(self.textures.images[index].clone());
                    self.texture_arcs.insert(index, image);
                }
                Arc::clone(&self.texture_arcs[&index])
            });
        if texture.is_none() {
            if let Some(warning) = self.textures.warnings.last() {
                println!("PLAYER TEXTURE WARNING: {warning}");
            }
        }
        (texture, alpha_mode)
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
            [0.0; 3], [1.0, 0.0, 0.0], 0.0, 3.0, 4, 1.0, 7, 0, 0, 0, 0,
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
                            .present_player_entity(&followed, &info, snapshot.server_time, 1.0, None, false, true)
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
                                PlayerFxRequest::SaberBlade { .. } => "saber blade".into(),
                                PlayerFxRequest::PushPuffs { .. } => "push puffs".into(),
                                PlayerFxRequest::GripPuffs { .. } => "grip puffs".into(),
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
) -> i32 {
    if info.definition_saber_colors { blade.color } else { client_color }
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

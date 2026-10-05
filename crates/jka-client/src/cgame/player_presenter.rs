//! cgame::player_presenter module facade. Implementations are grouped by responsibility.
mod assets;
mod attachments;
mod bodies;
mod cloth;
mod cosmetics;
mod diagnostics;
mod effects;
mod events;
mod physics;
mod pose;
mod presentation;
mod previews;
mod sabers;
mod state;
mod submission;
mod surface_materials;
mod vehicles;
mod view;
#[cfg(test)]
use assets::loading::{material_layers, resolved_stage};
use assets::loading::{
    specular_alpha, stage_uv_xform, wave_value, ModelLoadShared, ModelResolution, StageFrame,
};
use assets::types::{
    fpls_mode3_surface_hidden, Ghoul2GpuMeshSource, PlayerModelAsset, PlayerSurfaceAsset,
    ResolvedMaterial, ResolvedStage, SaberModelAsset, SurfaceOverlay,
};
#[cfg(test)]
use attachments::WP_MELEE;
use attachments::{
    body_queue_model1_weapon, reset_remote_player_saber_attachment, update_weapon_attachment,
    DEFAULT_PLAYER_MAXS, DEFAULT_PLAYER_MINS, DEFAULT_SABER_BLADE_LENGTH_MAX, DEFAULT_VIEWHEIGHT,
    EF_BODYPUSH, EF_JETPACK, FORCE_LEVEL_2, FORCE_LEVEL_3, FP_GRIP, FP_PROTECT, FP_RAGE,
    MAX_CLIENTS, MAX_GRIP_DISTANCE, MAX_SABER_BLADES, PUSH_BONE_NAMES, PW_DISINT_4, TEAM_SPECTATOR,
};
pub use attachments::{CosmeticDraw, PlayerFxRequest};
use bodies::dismemberment::{
    dismember_source_entity, dismember_source_ready, source_dismember_surface_set,
    BodyQueueCopyState, DetachedLimbVisual, DismemberPart, DismemberSourceSnap,
};
pub use cosmetics::apply_profile_studio_light;
use cosmetics::{cosmetic_draws_for_mask, model_bolt_matrix_timed, VehicleSnap};
pub use diagnostics::Ghoul2PerfStats;
use diagnostics::{record_motion_pose_eval, record_pose_eval};
#[cfg(test)]
use effects::glm_indices_for_wgpu;
use effects::{
    angles_to_axis, blade_color, flatten_matrix3x4, force_grip_target_alive,
    force_grip_target_still_valid, impulse_weapon_is_explosive, infer_force_grip_target,
    normalize_vec3, openjk_look_target_origin, openjk_player_entity_needs_reset,
    player_angle_entity, player_entity_rgb, presented_entity_in_knockdown,
    presented_entity_velocity, saber_blade_fx_request, transform_jka_model_point,
    transform_jka_model_vector, transform_model_normal, transform_model_point, viewer_style_from,
    weapon_world_model,
};
pub(super) use effects::{blend_for_alpha, openjk_vectoangles, saber_name_is_removed};
#[cfg(test)]
use effects::{custom_rgba_tint, head_sprite_shader, ray_default_player_box_entry};
use physics::{ActiveImpulseRagdoll, ExplosionImpulsePulse, PendingImpulseRagdoll, ANIM_TOGGLEBIT};
use pose::{foot_bolts_timed, model_bolt_origin_timed, skin_surface_timed};
use sabers::state::{EntityPlayerState, SaberBladeLengthKey, SaberBladeLengthState};
use state::resolve_vehicle_model_request;
use submission::mesh::{
    build_lod_gpu_meshes, gpu_bones_from_pose, is_asset_pending, player_model_key,
    sibling_default_skin_qpath, static_model_key, ASSET_PENDING,
};
use view::{
    ghoul2_entity_radius, ghoul2_lod_for_view, ghoul2_model_scale, ghoul2_render_origin,
    ghoul2_render_transform, BLOB_SHADOW_DISTANCE, BLOB_SHADOW_DROID_RADIUS, BLOB_SHADOW_MAXS,
    BLOB_SHADOW_MINS, BLOB_SHADOW_RADIUS, CLASS_REMOTE, CLASS_SEEKER, EF2_SHIP_DEATH, EF_DEAD,
    EF_NODRAW, EF_RAG, EF_TELEPORT_BIT, G2_MODELPART_HEAD, G2_MODELPART_LARM, G2_MODELPART_LLEG,
    G2_MODELPART_RARM, G2_MODELPART_RHAND, G2_MODELPART_RLEG, G2_MODELPART_WAIST, G2_MODEL_PART,
    MASK_PLAYERSOLID, MAX_GLA_BYTES, MAX_GLM_BYTES, MAX_JIGGLE_BYTES, MAX_SKIN_BYTES, PW_CLOAKED,
    WP_BRYAR_PISTOL, WP_SABER,
};
pub use view::{Ghoul2PresentationView, ViewerAnimDebug};

// Shared JKA player visual presentation for snapshot, followed, and local players.
//
// The data flow mirrors OpenJK's multiplayer cgame path: `ET_PLAYER` resolves
// `clientInfo_t` from `CS_PLAYERS`, registers `model.glm` + skin, runs the
// humanoid animation state, then submits Ghoul2 surfaces through the selected
// CPU/worker/GPU skinning backend. Snapshot ownership stays in `cgame.rs` so a
// network, demo, and local/offline sources use this exact same presentation code.

use super::cloth::{
    ClothCapsule, ClothConfig, ClothMotion, ClothOutput, ClothSurfaceFrame, ClothSystem,
};
use super::footsteps::{self, FootstepImpact, FootstepStages};
use super::jiggle::{JiggleProfile, JiggleSystem};
use super::player_animation::PlayerAnimationState;
use super::ragdoll::{
    DetachedLimbGeneration, DetachedLimbSpawn, PhysicsMapMesh, RagdollConfig, RagdollMode,
    RagdollWorld,
};
use super::saber_throw::{blade_angles, SaberThrowState};
use crate::{
    asset_jobs::{self, AssetPriority, AssetRegistry, AssetSource, AssetState, Requested},
    cgame::{
        suppressed_during_intermission, ClientGameState, ForcedPlayerModels, Ghoul2ServerCommand,
        PresentationEvent, PresentedEntity, ET_BODY, ET_GENERAL, ET_NPC, ET_PLAYER, GT_SIEGE,
    },
    materials::{self, TextureData, Textures},
    renderer::{
        DynamicModelAlphaMode, DynamicModelSurface, DynamicModelVertex, DynamicWireframeClass,
        FxGpuSpriteInstance, FxGpuSprites, Ghoul2GpuBone, Ghoul2GpuSkinning, Ghoul2GpuVertex,
    },
    scene,
    ui::Ghoul2SkinningMode,
};
use glam::Vec3;
use jka_assets::{
    animation::{animation_index, is_death_animation, load_humanoid_animations, AnimationSet},
    ghoul2::{
        model_bolt_matrix, multiply_3x4, openjk_default_gla, parse_gla, parse_glm,
        skin_glm_surface, smooth_ghoul2_pose, Ghoul2Animator, Ghoul2SkinnedSurface, GlaAnimation,
        GlmModel, GlmSurface, Matrix3x4, BONE_ANIM_OVERRIDE_FREEZE, BONE_ANIM_OVERRIDE_LOOP,
        OPENJK_DEFAULT_GLA_NAME,
    },
    pk3::AssetSearchPath,
    saber::{
        load_saber_animation_scales, load_saber_definitions, SaberAnimationScales, SaberDefinition,
        SaberDefinitions,
    },
    shader::{AlphaGen, Bulge, RgbGen, Shader, Stage, TcGen, TcMod, Wave, WaveFunc},
    siege::find_siege_class_visual,
    skin::load_skin,
    vehicle::{load_vehicle_definitions, VehicleDefinition, VehicleDefinitions},
};
use jka_movement::{
    bg_g2_player_angles, CollisionWorld, PlayerAngleEntity, PlayerAngleState, TraceQuery,
    BONE_ANGLES_POSTMULT,
};
use jka_protocol::entity_event::EntityEvent;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};

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

#[cfg(test)]
#[path = "player_presenter/tests/head_sprite_tests.rs"]
mod head_sprite_tests;

#[cfg(test)]
#[path = "player_presenter/tests/tests.rs"]
mod tests;

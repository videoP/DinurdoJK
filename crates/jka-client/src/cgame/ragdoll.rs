//! Rapier-backed client-side ragdolls for JKA Ghoul2 corpses.
//!
//! OpenJK's old Broadsword path enters ragdoll presentation for dead entities /
//! `EF_RAG` immediately before Ghoul2 submission. This module keeps that
//! ownership boundary, but replaces Broadsword's renderer-side solver with
//! Rapier.  Network snapshots and OpenJK pmove remain authoritative.

use crate::cgame::{weapon_fx::{ChunkModel, ChunkSpec, ShardSpec}, PresentedEntity};
use crate::fx::system::FxDraw;
use jka_assets::ghoul2::{multiply_3x4, GlaAnimation, Matrix3x4};
use glam::Mat3;
use rapier3d::{
    math::{Matrix, Pose, Rotation, Vector},
    prelude::*,
};
use std::collections::{HashMap, HashSet};

/// JKA uses large Quake-style map units. Keeping Rapier near meter-ish scales
/// improves solver tolerances without changing anything visible to the game.
const JKA_TO_RAPIER: f32 = 1.0 / 32.0;
const RAPIER_TO_JKA: f32 = 1.0 / JKA_TO_RAPIER;
const LIVING_RAGDOLL_RECOVERY_MS: i32 = 180;
const KNOCKDOWN_RAGDOLL_RECOVERY_MS: i32 = 240;
const JKA_GRAVITY: f32 = 800.0;
// Volko61/better-ragdoll (solver.py Params, GPL-3.0-or-later): exact
// angular spring / damping coefficients, in seconds and SI units.
// Rapier's position solver remains responsible for anchors and collision;
// spring angular XPBD corrections are mapped to equal/opposite impulses.
const JOINT_TENSION: f32 = 10.0;
const JOINT_TENSION_DAMPING: f32 = 0.35;
const JOINT_SOFT_ZONE: f32 = 0.25;
const JOINT_SOFT_STRENGTH: f32 = 400.0;
const JOINT_SOFT_DAMPING: f32 = 30.0;
const JOINT_BASE_DAMPING: f32 = 0.5;

#[derive(Debug, Clone, Copy)]
pub struct RagdollConfig {
    /// Master client visual-physics switch (`r_physics`).
    pub physics_enabled: bool,
    /// Articulated ragdolls specifically (`r_ragdolls`).
    pub enabled: bool,
    pub hz: u32,
    pub max_substeps: u32,
    pub ccd: bool,
    pub sleeping: bool,
    pub debris_enabled: bool,
    pub max_debris: u32,
    pub debris_lifetime_seconds: f32,
    pub max_ragdolls: u32,
    pub lifetime_seconds: f32,
    /// OpenJK cg_dismember semantics: 0 off, 1 limbs only, 2 full (head/waist too).
    pub dismemberment: u8,
    pub max_detached_limbs: u32,
    pub detached_limb_lifetime_seconds: f32,
    pub self_collision: bool,
    /// 0 = historical death-pose hard limits; 1 = same stable joint geometry
    /// with bounded progressive soft-limit resistance and bind-informed tension.
    pub joint_model: u8,
    pub weapon_impulses: bool,
    pub explosion_impulses: bool,
    pub force_impulses: bool,
    pub debug: bool,
    pub stats: bool,
}

impl Default for RagdollConfig {
    fn default() -> Self {
        Self {
            physics_enabled: false,
            enabled: false,
            hz: 60,
            max_substeps: 4,
            ccd: true,
            sleeping: true,
            debris_enabled: false,
            max_debris: 192,
            debris_lifetime_seconds: 10.0,
            max_ragdolls: 8,
            lifetime_seconds: 20.0,
            dismemberment: 0,
            max_detached_limbs: 24,
            detached_limb_lifetime_seconds: 16.0,
            self_collision: false,
            joint_model: 0,
            weapon_impulses: true,
            explosion_impulses: true,
            force_impulses: true,
            debug: false,
            stats: false,
        }
    }
}

#[derive(Clone, Default)]
pub struct PhysicsMapMesh {
    /// The expensive Parry/Rapier BVH is constructed on the map-prep worker.
    /// Installing this into a PlayerPresenter is therefore just an Arc clone.
    shape: Option<SharedShape>,
    vertex_count: usize,
    triangle_count: usize,
}

impl PhysicsMapMesh {
    #[cfg(test)]
    pub(crate) fn triangle_data(&self) -> Option<(Vec<[f32; 3]>, Vec<[u32; 3]>)> {
        let mesh = self.shape.as_ref()?.as_trimesh()?;
        Some((mesh.vertices().iter().map(|v| [v.x, v.y, v.z]).collect(), mesh.indices().to_vec()))
    }

    pub fn from_jka_mesh(
        vertices: Vec<[f32; 3]>,
        triangles: Vec<[u32; 3]>,
    ) -> Result<Self, String> {
        if vertices.is_empty() || triangles.is_empty() {
            return Ok(Self::default());
        }
        let vertex_count = vertices.len();
        let triangle_count = triangles.len();
        let vertices = vertices
            .into_iter()
            .map(|p| Vector::new(p[0], p[1], p[2]) * JKA_TO_RAPIER)
            .collect::<Vec<_>>();
        let shape = SharedShape::trimesh(vertices, triangles)
            .map_err(|error| format!("Rapier map trimesh: {error}"))?;
        Ok(Self {
            shape: Some(shape),
            vertex_count,
            triangle_count,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct BoneSpec {
    name: &'static str,
    end: Option<&'static str>,
    radius: f32,
    /// Approximate segment mass for a 1.0-scale humanoid. Rapier uses this
    /// to give the articulation human-like inertia instead of the default
    /// 1 kg/m^3 collider density (which makes these small meter-scaled
    /// capsules effectively weightless).
    mass_kg: f32,
}

// Use the stock JKA humanoid bones exposed by Broadsword's shared rag-effector
// enum, but only create rigid bodies for the major visible segments. Undriven
// Ghoul2 descendants inherit the solved transform of their nearest driven
// ancestor below.
const BODY_SPECS: &[BoneSpec] = &[
    BoneSpec { name: "pelvis", end: Some("lower_lumbar"), radius: 7.0, mass_kg: 10.0 },
    BoneSpec { name: "lower_lumbar", end: Some("upper_lumbar"), radius: 7.0, mass_kg: 6.0 },
    BoneSpec { name: "upper_lumbar", end: Some("thoracic"), radius: 6.5, mass_kg: 6.0 },
    BoneSpec { name: "thoracic", end: Some("cranium"), radius: 6.5, mass_kg: 14.0 },
    BoneSpec { name: "cranium", end: None, radius: 5.5, mass_kg: 5.0 },
    BoneSpec { name: "rhumerus", end: Some("rradius"), radius: 3.2, mass_kg: 2.2 },
    BoneSpec { name: "rradius", end: Some("rhand"), radius: 2.8, mass_kg: 1.5 },
    BoneSpec { name: "rhand", end: None, radius: 2.8, mass_kg: 0.6 },
    BoneSpec { name: "lhumerus", end: Some("lradius"), radius: 3.2, mass_kg: 2.2 },
    BoneSpec { name: "lradius", end: Some("lhand"), radius: 2.8, mass_kg: 1.5 },
    BoneSpec { name: "lhand", end: None, radius: 2.8, mass_kg: 0.6 },
    BoneSpec { name: "rfemurYZ", end: Some("rtibia"), radius: 4.2, mass_kg: 7.0 },
    BoneSpec { name: "rtibia", end: Some("rtalus"), radius: 3.4, mass_kg: 4.0 },
    BoneSpec { name: "rtalus", end: Some("rtarsal"), radius: 3.0, mass_kg: 1.0 },
    BoneSpec { name: "rtarsal", end: None, radius: 2.6, mass_kg: 1.0 },
    BoneSpec { name: "lfemurYZ", end: Some("ltibia"), radius: 4.2, mass_kg: 7.0 },
    BoneSpec { name: "ltibia", end: Some("ltalus"), radius: 3.4, mass_kg: 4.0 },
    BoneSpec { name: "ltalus", end: Some("ltarsal"), radius: 3.0, mass_kg: 1.0 },
    BoneSpec { name: "ltarsal", end: None, radius: 2.6, mass_kg: 1.0 },
];

#[derive(Debug, Clone, Copy)]
struct DrivenBone {
    bone_index: usize,
    body: RigidBodyHandle,
    /// Rigid (orthonormal) model-space bone transform at the exact handoff.
    /// Rapier necessarily operates on rigid transforms, while Ghoul2's
    /// component-wise animation interpolation may leave tiny scale/shear in
    /// the evaluated skinning matrices. Comparing against this rigid reference
    /// lets us apply only Rapier's *delta* to the exact frozen Ghoul2 pose.
    spawn_model_rigid: Matrix3x4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RagdollMode {
    Corpse,
    ForceGrip,
    /// Living articulated reaction with the thoracic body kinematically
    /// attached to the authoritative player transform. Limbs/head remain
    /// dynamic and therefore lag/react to server-owned knockback.
    Impulse,
}

type RagdollKind = RagdollMode;

#[derive(Debug, Clone, Copy)]
struct GripAnchor {
    /// Position-based kinematic body driven by the authoritative victim entity.
    body: RigidBodyHandle,
    /// Frozen cervical/neck position in unscaled Ghoul2 model space.
    neck_model_position: [f32; 3],
}

#[derive(Debug, Clone, Copy)]
struct TorsoAnchor {
    /// The thoracic rigid body itself is kinematic for impulse ragdolls.
    body: RigidBodyHandle,
    /// Frozen unscaled model-space thoracic transform at handoff.
    torso_model_rigid: Matrix3x4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CorpseGeneration {
    client_num: i32,
    /// Body-queue slots are reused, so ET_BODY needs one stable copy stamp.
    /// Live dead ET_PLAYER/ET_NPC entities deliberately use zero here: their
    /// server trajectory keeps changing while the same death is in progress.
    body_queue_time: i32,
}

// A reliable `ircg` BodyQueueCopy arrives at the exact OpenJK handoff point,
// before the destination ET_BODY's trajectory stamp necessarily reaches the
// presenter. Keep the already-solved Rapier corpse alive under the body-queue
// entity number and adopt the real stamp on its first ET_BODY presentation.
const BODY_QUEUE_PENDING_TIME: i32 = i32::MIN;

#[derive(Debug, Clone)]
struct LivingRagdollRecovery {
    model_key: String,
    started_at: i32,
    duration_ms: i32,
    from_pose: Vec<Matrix3x4>,
}

/// Anatomical joint description from the Blender Better Ragdoll model:
/// parent/child frames define flexion (+X), abduction (+Z), axial twist (+Y).
/// Its hard-limit constraint stays in Rapier; these are the separate soft
/// angular corrections and velocity damping, NOT motor targets.
#[derive(Debug, Clone, Copy)]
struct SoftJoint {
    parent: RigidBodyHandle,
    child: RigidBodyHandle,
    frame_p: Rotation,
    frame_c: Rotation,
    /// (minimum, maximum, neutral) in radians, mirrored for right limbs.
    ranges: [[f32; 3]; 3],
    /// Sum of subtree mass * distance-to-joint squared plus link inertia.
    effective_inertia: f32,
}

#[derive(Debug)]
struct RagdollInstance {
    model_key: String,
    model_scale: f32,
    generation: CorpseGeneration,
    kind: RagdollKind,
    grip_anchor: Option<GripAnchor>,
    torso_anchor: Option<TorsoAnchor>,
    spawned_at: i32,
    lifetime_slept: bool,
    /// Last authoritative/interpolated entity origin supplied by cgame. Rapier
    /// owns the articulation, but the server remains authoritative for gross
    /// corpse displacement (Force push, knockback, movers, etc.).
    last_authoritative_origin: [f32; 3],
    /// Exact frozen Ghoul2 skinning pose captured at the physics handoff.
    /// Keep these matrices byte-for-byte as the visual reference. Rapier only
    /// contributes rigid deltas on top, so entering ragdoll cannot rescale or
    /// otherwise re-normalize the character's skinning pose.
    spawn_pose: Vec<Matrix3x4>,
    /// Most recent fully solved Ghoul2 pose actually submitted for this
    /// ragdoll. Living ragdoll release captures this exact visual pose and blends
    /// it back to live animation instead of snapping on the first non-grip
    /// frame.
    last_applied_pose: Vec<Matrix3x4>,
    driven: Vec<DrivenBone>,
    soft_joints: Vec<SoftJoint>,
}

/// Stable identity of one server-authored `G2_MODEL_PART` presentation entity.
/// Entity numbers are reusable, so the source/kind/trTime tuple is part of the
/// generation just like ET_BODY queue slots use a copy timestamp.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DetachedLimbGeneration {
    pub source_entity: u16,
    pub kind: u8,
    pub trajectory_time: i32,
}

/// Spawn data prepared by the Ghoul2 presenter at the exact sever pose.
/// All coordinates are JKA world/model units except angular velocity (rad/s).
#[derive(Debug, Clone, Copy)]
pub(crate) struct DetachedLimbSpawn {
    pub entity: u16,
    pub generation: DetachedLimbGeneration,
    pub body_world: Matrix3x4,
    /// Capsule endpoints in the rigid body's local frame. `None` creates a ball.
    pub capsule_local: Option<([f32; 3], [f32; 3])>,
    pub radius: f32,
    pub mass_kg: f32,
    pub linear_velocity: [f32; 3],
    pub angular_velocity: [f32; 3],
}

#[derive(Debug)]
struct DetachedLimbBody {
    generation: DetachedLimbGeneration,
    body: RigidBodyHandle,
    spawned_at: i32,
}

/// One server-authored EV_DEBRIS model, simulated in the existing Rapier
/// world. No server entity or gameplay collision is created.
#[derive(Debug)]
struct DebrisBody {
    body: RigidBodyHandle,
    qpath: String,
    scale: f32,
    expires_at: i32,
}

/// Glass is still rendered as the original FX quad. Only its transform is
/// driven by Rapier. A delayed shard has no body until the TaystJK start time.
#[derive(Debug)]
struct GlassBody {
    body: Option<RigidBodyHandle>,
    spec: ShardSpec,
    centre: [f32; 3],
    born_at: i32,
    activate_at: i32,
    expires_at: i32,
}

/// Presentation-only player hull in JKA world coordinates.
pub(crate) struct PhysicsPlayer {
    pub entity: u16,
    pub origin: [f32; 3],
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub teleport_bit: i32,
}

struct PlayerCollider {
    body: RigidBodyHandle,
    collider: ColliderHandle,
    target: Vector,
    half_extents: Vector,
    teleport_bit: i32,
    sampled_at: i32,
}

pub struct RagdollWorld {
    config: RagdollConfig,
    pipeline: PhysicsPipeline,
    integration: IntegrationParameters,
    islands: IslandManager,
    broad_phase: BroadPhaseBvh,
    narrow_phase: NarrowPhase,
    bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    ccd_solver: CCDSolver,
    static_world_ready: bool,
    previous_body_poses: HashMap<RigidBodyHandle, Pose>,
    instances: HashMap<u16, RagdollInstance>,
    player_colliders: HashMap<u16, PlayerCollider>,
    detached_limbs: HashMap<u16, DetachedLimbBody>,
    debris: Vec<DebrisBody>,
    glass: Vec<GlassBody>,
    living_recoveries: HashMap<u16, LivingRagdollRecovery>,
    retired: HashMap<u16, (String, CorpseGeneration)>,
    last_time: Option<i32>,
    accumulator: f32,
    last_stats_print: Option<i32>,
    debug_candidates: HashMap<u16, CorpseGeneration>,
}

impl Default for RagdollWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl RagdollWorld {
    pub fn new() -> Self {
        Self {
            config: RagdollConfig::default(),
            pipeline: PhysicsPipeline::new(),
            integration: IntegrationParameters::default(),
            islands: IslandManager::new(),
            broad_phase: BroadPhaseBvh::new(),
            narrow_phase: NarrowPhase::new(),
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            static_world_ready: false,
            previous_body_poses: HashMap::new(),
            instances: HashMap::new(),
            player_colliders: HashMap::new(),
            detached_limbs: HashMap::new(),
            debris: Vec::new(),
            glass: Vec::new(),
            living_recoveries: HashMap::new(),
            retired: HashMap::new(),
            last_time: None,
            accumulator: 0.0,
            last_stats_print: None,
            debug_candidates: HashMap::new(),
        }
    }

    pub fn active(&self) -> bool {
        self.config.enabled && self.static_world_ready
    }

    pub fn dismemberment_level(&self) -> u8 {
        self.config.dismemberment
    }

    fn detached_physics_active(&self) -> bool {
        self.config.physics_enabled && self.config.dismemberment != 0 && self.static_world_ready
    }

    fn debris_active(&self) -> bool {
        self.config.physics_enabled && self.config.debris_enabled && self.static_world_ready
    }

    fn world_active(&self) -> bool {
        // Do not step an otherwise-unused Rapier world merely because
        // cg_dismember is enabled. The first detached body is seeded after the
        // presentation prepass and starts integrating on the following frame.
        self.active() || (self.detached_physics_active() && !self.detached_limbs.is_empty())
            || (self.debris_active() && (!self.debris.is_empty() || self.glass.iter().any(|g| g.body.is_some())))
    }

    pub fn debug_enabled(&self) -> bool {
        self.config.debug
    }

    pub fn weapon_impulses_enabled(&self) -> bool {
        self.config.weapon_impulses
    }

    pub fn explosion_impulses_enabled(&self) -> bool {
        self.config.explosion_impulses
    }

    pub fn force_impulses_enabled(&self) -> bool {
        self.config.force_impulses
    }

    /// Drop only dynamic ragdoll state; keep the map collider and configuration.
    pub fn reset_dynamic_for_seek(&mut self) {
        self.clear_player_colliders();
        self.clear_instances();
        self.clear_detached_limbs();
        self.clear_debris();
        self.clear_glass();
        self.previous_body_poses.clear();
        self.living_recoveries.clear();
        self.last_time = None;
        self.accumulator = 0.0;
    }

    pub fn set_config(&mut self, config: RagdollConfig) {
        let was_enabled = self.config.enabled;
        let debris_was_enabled = self.debris_active();
        let rebuild_debris = self.config.ccd != config.ccd || self.config.sleeping != config.sleeping;
        let physics_was_enabled = self.config.physics_enabled;
        let debug_was = self.config.debug;
        let rebuild_ragdolls = self.config.enabled
            && config.enabled
            && (self.config.ccd != config.ccd
                || self.config.sleeping != config.sleeping
                || self.config.self_collision != config.self_collision
                // Existing joints are physically different. Keeping old ones
                // made selecting Classic appear ineffective after using mode 1.
                || self.config.joint_model != config.joint_model);
        let rebuild_detached = self.config.physics_enabled
            && config.physics_enabled
            && self.config.dismemberment != 0
            && config.dismemberment != 0
            && (self.config.ccd != config.ccd || self.config.sleeping != config.sleeping);
        self.config = RagdollConfig {
            hz: config.hz.clamp(30, 240),
            max_substeps: config.max_substeps.clamp(1, 16),
            max_ragdolls: config.max_ragdolls.min(64),
            max_debris: config.max_debris.min(768),
            debris_lifetime_seconds: config.debris_lifetime_seconds.clamp(1.0, 300.0),
            lifetime_seconds: config.lifetime_seconds.clamp(0.0, 300.0),
            dismemberment: config.dismemberment.min(2),
            joint_model: config.joint_model.min(1),
            max_detached_limbs: config.max_detached_limbs.min(128),
            detached_limb_lifetime_seconds: config.detached_limb_lifetime_seconds.clamp(0.0, 300.0),
            ..config
        };

        if self.config.debug && (!debug_was || was_enabled != self.config.enabled) {
            self.debug_candidates.clear();
            println!(
                "RAPIER DEBUG: enabled={} mapReady={} ragdolls={} bodies={} colliders={} joints={} hz={} maxSubsteps={}",
                self.config.enabled,
                self.static_world_ready,
                self.instances.len(),
                self.bodies.len(),
                self.colliders.len(),
                self.impulse_joints.len(),
                self.config.hz,
                self.config.max_substeps,
            );
        }

        // A disabled visual-physics feature must not keep stale solved corpse
        // poses around to jump back to when it is toggled on again. The map
        // collider remains installed; only dynamic ragdoll instances are reset.
        if was_enabled && !self.config.enabled {
            self.clear_instances();
            self.clear_player_colliders();
        }
        if physics_was_enabled && !self.config.physics_enabled {
            self.clear_detached_limbs();
        }
        if !was_enabled && self.config.enabled {
            self.last_time = None;
            self.accumulator = 0.0;
        }
        // These flags are baked into already-created rigid bodies/colliders.
        // Re-seed visible corpses from their current authoritative animation
        // pose instead of leaving a mixed old/new physics world.
        if rebuild_ragdolls {
            self.clear_instances();
        }
        if rebuild_detached {
            self.clear_detached_limbs();
        }
        if self.config.dismemberment == 0 {
            self.clear_detached_limbs();
        }
        if debris_was_enabled && (!self.debris_active() || rebuild_debris) {
            self.clear_debris();
            self.clear_glass();
        }
        self.trim_to_budget();
        self.trim_detached_to_budget();
        self.trim_debris_to_budget();
    }

    /// Replace the fixed Rapier world collider for a newly loaded map.
    /// The mesh is authored in JKA coordinates and scaled only inside Rapier.
    pub fn set_map_mesh(&mut self, mesh: &PhysicsMapMesh) -> Result<(), String> {
        self.reset_world();
        self.debug_candidates.clear();
        let Some(shape) = mesh.shape.clone() else {
            if self.config.debug {
                println!("RAPIER MAP COLLISION: unavailable (physics mesh was not prepared for this map)");
            }
            return Ok(());
        };

        let builder = ColliderBuilder::new(shape);
        let static_groups =
            InteractionGroups::new(Group::GROUP_1, Group::GROUP_2, InteractionTestMode::default());
        self.colliders.insert(
            builder
                .friction(0.9)
                .restitution(0.0)
                .collision_groups(static_groups),
        );
        self.static_world_ready = true;
        devprintln!(
            1,
            "RAPIER MAP COLLISION: {} vertices / {} triangles",
            mesh.vertex_count,
            mesh.triangle_count
        );
        Ok(())
    }

    /// Players are kinematic in this visual world: they transfer motion to
    /// corpses without feeding any forces back into authoritative pmove.
    pub(crate) fn set_players(&mut self, players: &[PhysicsPlayer], current_time: i32) {
        if !self.active() {
            self.clear_player_colliders();
            return;
        }
        let mut live = HashSet::new();
        for player in players {
            if !player.origin.iter().chain(&player.mins).chain(&player.maxs)
                .all(|value| value.is_finite()) {
                continue;
            }
            let mins = Vector::from_array(player.mins);
            let maxs = Vector::from_array(player.maxs);
            let half_extents = (maxs - mins) * (0.5 * JKA_TO_RAPIER);
            if half_extents.min_element() <= 0.0 { continue; }
            let target = (Vector::from_array(player.origin) + (mins + maxs) * 0.5) * JKA_TO_RAPIER;
            live.insert(player.entity);
            if let Some(proxy) = self.player_colliders.get_mut(&player.entity) {
                let elapsed = current_time.saturating_sub(proxy.sampled_at);
                // Teleports, seeks and loading gaps must not sweep a hull
                // through every corpse between unrelated positions.
                if proxy.teleport_bit != player.teleport_bit || elapsed > 100 || elapsed < 0
                    || (target - proxy.target).length_squared() > (128.0 * JKA_TO_RAPIER).powi(2) {
                    if let Some(body) = self.bodies.get_mut(proxy.body) {
                        body.set_translation(target, true);
                        body.set_next_kinematic_translation(target);
                    }
                }
                if proxy.half_extents != half_extents {
                    self.colliders[proxy.collider].set_shape(SharedShape::cuboid(
                        half_extents.x, half_extents.y, half_extents.z,
                    ));
                }
                proxy.target = target;
                proxy.half_extents = half_extents;
                proxy.teleport_bit = player.teleport_bit;
                proxy.sampled_at = current_time;
                continue;
            }
            let body = self.bodies.insert(RigidBodyBuilder::kinematic_position_based().translation(target));
            let collider = self.colliders.insert_with_parent(
                ColliderBuilder::cuboid(half_extents.x, half_extents.y, half_extents.z)
                    .friction(0.0).restitution(0.0)
                    .collision_groups(InteractionGroups::new(
                        Group::GROUP_3, Group::GROUP_2, InteractionTestMode::default(),
                    )),
                body, &mut self.bodies,
            );
            self.player_colliders.insert(player.entity, PlayerCollider {
                body, collider, target, half_extents,
                teleport_bit: player.teleport_bit, sampled_at: current_time,
            });
        }
        let stale = self.player_colliders.keys().filter(|entity| !live.contains(entity))
            .copied().collect::<Vec<_>>();
        for entity in stale {
            if let Some(proxy) = self.player_colliders.remove(&entity) { self.remove_body(proxy.body); }
        }
    }

    fn clear_player_colliders(&mut self) {
        for (_, proxy) in std::mem::take(&mut self.player_colliders) {
            self.remove_body(proxy.body);
        }
    }

    pub fn begin_frame(&mut self, current_time: i32) {
        self.living_recoveries.retain(|_, recovery| {
            current_time.saturating_sub(recovery.started_at) < recovery.duration_ms
        });
        let Some(previous) = self.last_time.replace(current_time) else {
            return;
        };
        if current_time < previous {
            // Demo seeking/reverse playback invalidates every integrated pose.
            // The next ET_BODY presentation will seed each corpse from the
            // authoritative snapshot/death animation pose at the new time.
            self.clear_instances();
            self.clear_detached_limbs();
            self.clear_debris();
            self.clear_glass();
            self.clear_player_colliders();
            self.accumulator = 0.0;
            return;
        }
        if current_time == previous {
            // Paused demo/game time: keep the existing interpolation fraction
            // so the rendered corpse freezes exactly where it was.
            return;
        }

        self.sleep_expired(current_time);
        self.expire_debris(current_time);
        self.expire_glass(current_time);
        self.activate_glass(current_time);
        if !self.world_active() {
            self.accumulator = 0.0;
            return;
        }

        // The presentation clock can jump while loading/fast-forwarding. Never
        // try to pay back an arbitrarily large physics debt in one frame.
        let frame_dt = ((current_time - previous) as f32 * 0.001).min(0.1);
        let step_dt = 1.0 / self.config.hz.max(1) as f32;
        self.integration.dt = step_dt;
        self.accumulator += frame_dt;

        let mut steps = 0;
        let step_count = (((self.accumulator + 1.0e-6) / step_dt) as u32).min(self.config.max_substeps);
        let player_paths = self.player_colliders.values().filter_map(|proxy| {
            self.bodies.get(proxy.body).map(|body| (proxy.body, body.translation(), proxy.target))
        }).collect::<Vec<_>>();
        while self.accumulator + 1.0e-6 >= step_dt && steps < self.config.max_substeps {
            // Spread frame movement across fixed steps, including frames that
            // accumulated several render samples before the first physics tick.
            let fraction = (steps + 1) as f32 / step_count.max(1) as f32;
            for &(handle, start, target) in &player_paths {
                if let Some(body) = self.bodies.get_mut(handle) {
                    body.set_next_kinematic_translation(start + (target - start) * fraction);
                }
            }
            self.capture_previous_poses();
            // Classic stays byte-for-byte on its old solver path.
            if self.config.joint_model == 1 { self.update_soft_joint_springs(step_dt); }
            self.pipeline.step(
                Vector::new(0.0, 0.0, -JKA_GRAVITY * JKA_TO_RAPIER),
                &self.integration,
                &mut self.islands,
                &mut self.broad_phase,
                &mut self.narrow_phase,
                &mut self.bodies,
                &mut self.colliders,
                &mut self.impulse_joints,
                &mut self.multibody_joints,
                &mut self.ccd_solver,
                &(),
                &(),
            );
            if self.config.joint_model == 1 { self.update_soft_joint_damping(step_dt); }
            self.accumulator -= step_dt;
            steps += 1;
        }
        if steps == self.config.max_substeps && self.accumulator >= step_dt {
            self.accumulator = self.accumulator.min(step_dt);
        }

        if self.config.stats
            && self.last_stats_print.map_or(true, |last| current_time.saturating_sub(last) >= 1000)
        {
            self.last_stats_print = Some(current_time);
            println!(
                "RAPIER STATS: active={} ragdolls={} detachedLimbs={} debris={} glass={} bodies={} colliders={} joints={} steps={} accumulatorMs={:.2}",
                self.world_active(),
                self.instances.len(),
                self.detached_limbs.len(),
                self.debris.len(),
                self.glass.len(),
                self.bodies.len(),
                self.colliders.len(),
                self.impulse_joints.len(),
                steps,
                self.accumulator * 1000.0,
            );
        }
    }

    /// Position-constraint part of Better Ragdoll solver.py: neutral tension,
    /// then separate progressive boundary correction. Blender's XPBD solves
    /// these on positions; in Rapier the corresponding compliant angular
    /// correction is applied as paired velocity impulses ahead of integration.
    fn update_soft_joint_springs(&mut self, h: f32) {
        for instance in self.instances.values() {
            for joint in &instance.soft_joints {
                let Some((qp, qc, angles)) = soft_joint_angles(&self.bodies, joint) else { continue; };
                if joint_both_sleeping(&self.bodies, joint) { continue; }
                // _rotate_joint_to(neutral) in source solver, separate spring.
                apply_soft_joint_spring(&mut self.bodies, joint, qp, qc,
                    [joint.ranges[0][2], joint.ranges[1][2], joint.ranges[2][2]],
                    JOINT_TENSION * joint.effective_inertia, h);
                // Recompute angles after the neutral impulse only for damping:
                // unlike the original XPBD position solver, an impulse doesn't
                // rotate until Rapier integrates the next substep.
                let mut target = angles;
                let mut depth: f32 = 0.0;
                for (index, range) in joint.ranges.iter().enumerate() {
                    let width = (range[1] - range[0]) * JOINT_SOFT_ZONE;
                    if width <= 1.0e-6 { continue; }
                    if angles[index] > range[1] - width {
                        depth = depth.max(((angles[index] - (range[1] - width)) / width).min(1.0));
                        target[index] = range[1] - width;
                    } else if angles[index] < range[0] + width {
                        depth = depth.max((((range[0] + width) - angles[index]) / width).min(1.0));
                        target[index] = range[0] + width;
                    }
                }
                if depth > 0.0 {
                    apply_soft_joint_spring(&mut self.bodies, joint, qp, qc, target,
                        JOINT_SOFT_STRENGTH * joint.effective_inertia * depth, h);
                }
            }
        }
    }

    /// Velocity-constraint part of Better Ragdoll solver.py, executed AFTER
    /// Rapier's position/contact step, as in the original XPBD substep.
    fn update_soft_joint_damping(&mut self, h: f32) {
        for instance in self.instances.values() {
            for joint in &instance.soft_joints {
                if joint_both_sleeping(&self.bodies, joint) { continue; }
                let Some((qp, qc, angles)) = soft_joint_angles(&self.bodies, joint) else { continue; };
                let axes = [qp * Vector::X, qp * Vector::Z, qc * Vector::Y];
                for (index, axis) in axes.iter().copied().enumerate() {
                    let [lo, hi, _] = joint.ranges[index];
                    let width = (hi - lo) * JOINT_SOFT_ZONE;
                    let zone = if width > 1.0e-6 {
                        (((lo + width - angles[index]).max(0.0))
                            .max((angles[index] - (hi - width)).max(0.0)) / width).min(1.0)
                    } else { 0.0 };
                    let rate = JOINT_BASE_DAMPING
                        + 2.0 * JOINT_TENSION_DAMPING * JOINT_TENSION.sqrt()
                        + JOINT_SOFT_DAMPING * zone;
                    let inv = joint_inverse_inertia(&self.bodies, joint, axis);
                    if inv <= 1.0e-12 { continue; }
                    let (Some(parent), Some(child)) =
                        (self.bodies.get(joint.parent), self.bodies.get(joint.child)) else { continue; };
                    let relative_speed = (child.angvel() - parent.angvel()).dot(axis);
                    let chw = rate * joint.effective_inertia * h * inv;
                    let delta_speed = -relative_speed * chw / (1.0 + chw);
                    apply_joint_torque_impulse(&mut self.bodies, joint, axis * (delta_speed / inv));
                }
            }
        }
    }

    /// Spawn the corpse on first sight and rewrite `pose` with the latest
    /// Rapier solution. `axis` is the unscaled JKA entity orientation while
    /// `origin` includes the same modelScale floor correction as rendering.
    /// Rapier receives modelScale explicitly so rigid-body rotations stay unit
    /// length while body positions/collider dimensions still match the skin.
    pub fn apply_to_pose(
        &mut self,
        entity: &PresentedEntity,
        model_key: &str,
        gla: &GlaAnimation,
        pose: &mut [Matrix3x4],
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        model_scale: f32,
        mode: RagdollMode,
        initial_velocity_override: Option<[f32; 3]>,
        current_time: i32,
    ) -> Result<bool, String> {
        let generation = corpse_generation(entity);
        let kind = mode;
        // Any new physics takeover supersedes a pending live-animation
        // recovery for this entity (re-grip or death during recovery).
        self.living_recoveries.remove(&entity.number);
        if !self.active() || self.config.max_ragdolls == 0 {
            let first_report = self
                .debug_candidates
                .get(&entity.number)
                .is_none_or(|seen| *seen != generation);
            if self.config.debug && first_report {
                self.debug_candidates.insert(entity.number, generation);
                println!(
                    "RAPIER CANDIDATE REJECT: entity={} type={} eFlags={:#x} enabled={} mapReady={} maxRagdolls={}",
                    entity.number,
                    entity.entity_type,
                    entity.state.field_i32("eFlags").unwrap_or(0),
                    self.config.enabled,
                    self.static_world_ready,
                    self.config.max_ragdolls,
                );
            }
            return Ok(false);
        }
        if let Some((retired_model, retired_generation)) = self.retired.get(&entity.number) {
            if kind == RagdollKind::Corpse
                && retired_model == model_key
                && *retired_generation == generation
            {
                return Ok(false);
            }
            // Body-queue entity numbers are reused. A changed trajectory/model
            // means this slot now represents a new corpse and may enter the
            // active budget again.
            self.retired.remove(&entity.number);
        }

        if entity.entity_type == crate::cgame::ET_BODY {
            if let Some(instance) = self.instances.get_mut(&entity.number) {
                if instance.kind == RagdollKind::Corpse
                    && instance.generation.client_num == generation.client_num
                    && instance.generation.body_queue_time == BODY_QUEUE_PENDING_TIME
                {
                    // CG_BodyQueueCopy duplicates the already-dead Ghoul2
                    // instance; it does not restart the corpse's death pose.
                    // Rekeying the existing Rapier articulation is the same
                    // ownership transfer for this renderer.
                    instance.generation = generation;
                }
            }
        }

        let must_respawn = self
            .instances
            .get(&entity.number)
            .is_some_and(|instance| {
                instance.model_key != model_key
                    || instance.generation != generation
                    || instance.kind != kind
                    || (instance.model_scale - model_scale).abs() > 1.0e-4
                    || current_time < instance.spawned_at
            });
        if must_respawn {
            self.remove_instance(entity.number);
        }

        if !self.instances.contains_key(&entity.number) {
            if self.config.debug {
                self.debug_candidates.insert(entity.number, generation);
                println!(
                    "RAPIER CANDIDATE: entity={} kind={:?} type={} eFlags={:#x} clientNum={} model={} grounded={} trDelta=[{:.1},{:.1},{:.1}]",
                    entity.number,
                    kind,
                    entity.entity_type,
                    entity.state.field_i32("eFlags").unwrap_or(0),
                    entity.state.field_i32("clientNum").unwrap_or(i32::from(entity.number)),
                    model_key,
                    entity.state.field_i32("groundEntityNum").unwrap_or(1023) != 1023,
                    entity.state.field_f32("pos.trDelta[0]").unwrap_or(0.0),
                    entity.state.field_f32("pos.trDelta[1]").unwrap_or(0.0),
                    entity.state.field_f32("pos.trDelta[2]").unwrap_or(0.0),
                );
            }
            // Prefer the newest death when the visual budget is full. This is
            // presentation-only, so evicting the oldest solved corpse cannot
            // affect authoritative entity state.
            while self.instances.len() >= self.config.max_ragdolls as usize {
                let Some(oldest) = self.oldest_instance() else {
                    break;
                };
                self.retire_instance(oldest);
            }
            // Broadsword seeds a grounded corpse at rest and gives airborne
            // corpses extra carry-through from their entity trajectory. Keep
            // that behavior at the handoff into Rapier instead of inventing a
            // new death impulse here. ENTITYNUM_NONE is MAX_GENTITIES - 1.
            const ENTITYNUM_NONE: i32 = 1023;
            let grounded = entity
                .state
                .field_i32("groundEntityNum")
                .is_some_and(|ground| ground != ENTITYNUM_NONE);
            // Corpse handoff mirrors Broadsword's 2x airborne carry-through.
            // A living Force-grip victim already has authoritative server
            // velocity, so seed exactly that velocity and let the neck anchor
            // pull the articulation from there.
            let velocity_scale = match kind {
                RagdollKind::ForceGrip | RagdollKind::Impulse => 1.0,
                RagdollKind::Corpse if grounded => 0.0,
                RagdollKind::Corpse => 2.0,
            };
            let seed_velocity = initial_velocity_override.unwrap_or([
                entity.state.field_f32("pos.trDelta[0]").unwrap_or(0.0),
                entity.state.field_f32("pos.trDelta[1]").unwrap_or(0.0),
                entity.state.field_f32("pos.trDelta[2]").unwrap_or(0.0),
            ]);
            let velocity = Vector::new(seed_velocity[0], seed_velocity[1], seed_velocity[2])
                * (JKA_TO_RAPIER * velocity_scale);
            let instance = self.spawn_instance(
                model_key,
                gla,
                pose,
                axis,
                origin,
                model_scale,
                generation,
                kind,
                velocity,
                current_time,
            )?;
            if instance.driven.len() < 5 {
                // A non-humanoid GLA can legitimately lack Broadsword's bones.
                for driven in &instance.driven {
                    self.remove_body(driven.body);
                }
                if let Some(anchor) = instance.grip_anchor {
                    self.remove_body(anchor.body);
                }
                return Ok(false);
            }
            devprintln!(
                2,
                "RAPIER RAGDOLL SPAWN: entity={} kind={:?} model={} bodies={} modelScale={:.3}",
                entity.number,
                kind,
                model_key,
                instance.driven.len(),
                model_scale,
            );
            self.instances.insert(entity.number, instance);
        } else if kind == RagdollKind::Corpse {
            // The server can continue moving a dead ET_NPC/ET_PLAYER/ET_BODY
            // after Rapier has taken over the local articulation. OpenJK's
            // Broadsword lived in the Ghoul2 presentation path while cgame's
            // entity trajectory remained authoritative; preserve that split.
            //
            // Force-grip ragdolls are different: the server moves the living
            // victim origin, while Rapier should hang from the neck and react
            // to that motion instead of translating every segment rigidly.
            self.reconcile_authoritative_translation(entity.number, origin);
        }

        if kind == RagdollKind::ForceGrip {
            self.update_grip_anchor(entity.number, axis, origin, model_scale);
        } else if kind == RagdollKind::Impulse {
            self.update_torso_anchor(entity.number, axis, origin, model_scale);
        }

        let entity_matrix = entity_matrix(axis, origin);
        let entity_inverse = inverse_affine(&entity_matrix)
            .ok_or_else(|| format!("ragdoll entity {} has singular render transform", entity.number))?;
        // Never rebuild Ghoul2's skinning matrices from Rapier absolutes.
        // The evaluated pose is an exact visual object and may contain the tiny
        // non-rigid terms produced by Ghoul2's component-wise frame lerp. If we
        // orthonormalize those matrices and then reconstruct every descendant,
        // the result can visibly change scale and can explode helper/face bones.
        //
        // Instead, compute one rigid MODEL-SPACE delta for every simulated
        // segment and pre-multiply that delta onto the exact frozen skinning
        // matrix. At the handoff the delta is identity, so ragdoll takeover is
        // guaranteed not to change model size or the frozen death pose.
        let (spawn_pose, driven_bones) = self
            .instances
            .get(&entity.number)
            .map(|instance| (instance.spawn_pose.clone(), instance.driven.clone()))
            .unwrap_or_default();
        if spawn_pose.len() != pose.len() {
            return Ok(false);
        }

        let mut driven_deltas = HashMap::<usize, Matrix3x4>::new();
        for driven in &driven_bones {
            let Some(body) = self.bodies.get(driven.body) else {
                continue;
            };
            let current_pose = body.position();
            let previous_pose = self
                .previous_body_poses
                .get(&driven.body)
                .unwrap_or(current_pose);
            let interpolated = interpolate_pose(
                previous_pose,
                current_pose,
                self.interpolation_alpha(),
            );
            let world = pose_to_matrix(&interpolated);
            let scaled_model_bone = multiply_3x4(&entity_inverse, &world);
            let solved_model_rigid = scale_bone_translation(
                &scaled_model_bone,
                1.0 / model_scale.max(1.0e-6),
            );
            let Some(spawn_inverse) = inverse_affine(&driven.spawn_model_rigid) else {
                continue;
            };
            driven_deltas.insert(
                driven.bone_index,
                multiply_3x4(&solved_model_rigid, &spawn_inverse),
            );
        }

        // Broadsword treats several helper bones as explicit rag effectors even
        // though they are not useful independent rigid bodies. Bind those skin
        // helpers to the corresponding physical segment (not merely whatever
        // happens to be their GLA parent). This is especially important for
        // `ceyebrow`, which carries facial skin on the stock humanoid and is a
        // documented Broadsword effector.
        for index in 0..pose.len() {
            if let Some(driver) = ragdoll_driver_for_bone(gla, index, &driven_deltas) {
                pose[index] = multiply_3x4(&driven_deltas[&driver], &spawn_pose[index]);
            } else {
                pose[index] = spawn_pose[index];
            }
        }
        if let Some(instance) = self.instances.get_mut(&entity.number) {
            instance.last_applied_pose.clear();
            instance.last_applied_pose.extend_from_slice(pose);
        }
        Ok(true)
    }

    /// Begin a short visual recovery from the final living ragdoll pose to
    /// the authoritative live Ghoul2 animation. Gameplay state changes
    /// immediately; only presentation is blended.
    pub fn begin_living_recovery(&mut self, entity: u16, current_time: i32) {
        self.begin_living_recovery_for(entity, current_time, LIVING_RAGDOLL_RECOVERY_MS);
    }

    /// Knockdowns deserve a slightly softer handoff than ordinary grip/impulse
    /// release. The hidden Ghoul2 animation continues advancing while physics
    /// owns the pose, so this blends toward the authoritative knockdown frame
    /// that is current *now* rather than restarting the animation at frame 0.
    pub fn begin_knockdown_recovery(&mut self, entity: u16, current_time: i32) {
        self.begin_living_recovery_for(entity, current_time, KNOCKDOWN_RAGDOLL_RECOVERY_MS);
    }

    pub fn has_instance(&self, entity: u16) -> bool {
        self.instances.contains_key(&entity)
    }

    fn begin_living_recovery_for(&mut self, entity: u16, current_time: i32, duration_ms: i32) {
        let Some(instance) = self.instances.get(&entity) else {
            return;
        };
        if !matches!(instance.kind, RagdollKind::ForceGrip | RagdollKind::Impulse)
            || instance.last_applied_pose.is_empty()
        {
            return;
        }
        let recovery = LivingRagdollRecovery {
            model_key: instance.model_key.clone(),
            started_at: current_time,
            duration_ms,
            from_pose: instance.last_applied_pose.clone(),
        };
        self.living_recoveries.insert(entity, recovery);
        self.remove_instance(entity);
        if self.config.debug {
            println!(
                "RAPIER LIVING RAGDOLL RECOVERY: victim={} durationMs={}",
                entity, duration_ms
            );
        }
    }

    /// Blend the exact final rendered ragdoll pose into this frame's normal
    /// living animation. Ghoul2 itself uses component-wise 3x4 matrix lerps
    /// for animation blends; use the same convention here for a brief handoff.
    pub fn apply_living_recovery(
        &mut self,
        entity: u16,
        model_key: &str,
        pose: &mut [Matrix3x4],
        current_time: i32,
    ) -> bool {
        let Some(recovery) = self.living_recoveries.get(&entity).cloned() else {
            return false;
        };
        if recovery.model_key != model_key || recovery.from_pose.len() != pose.len() {
            self.living_recoveries.remove(&entity);
            return false;
        }

        let elapsed = current_time.saturating_sub(recovery.started_at);
        if elapsed >= recovery.duration_ms {
            self.living_recoveries.remove(&entity);
            return false;
        }
        let t = (elapsed.max(0) as f32 / recovery.duration_ms as f32).clamp(0.0, 1.0);
        let blend = t * t * (3.0 - 2.0 * t);
        for (target, from) in pose.iter_mut().zip(recovery.from_pose.iter()) {
            *target = lerp_matrix3x4(from, target, blend);
        }
        true
    }

    pub fn retain_visible(&mut self, visible_bodies: &HashSet<u16>) {
        let stale = self
            .instances
            .keys()
            .copied()
            .filter(|entity| !visible_bodies.contains(entity))
            .collect::<Vec<_>>();
        for entity in stale {
            self.remove_instance(entity);
        }
        self.retired
            .retain(|entity, _| visible_bodies.contains(entity));
        self.debug_candidates
            .retain(|entity, _| visible_bodies.contains(entity));
    }

    fn spawn_instance(
        &mut self,
        model_key: &str,
        gla: &GlaAnimation,
        pose: &[Matrix3x4],
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        model_scale: f32,
        generation: CorpseGeneration,
        kind: RagdollKind,
        velocity: Vector,
        current_time: i32,
    ) -> Result<RagdollInstance, String> {
        let entity_matrix = entity_matrix(axis, origin);
        let spawn_pose = pose.to_vec();
        let spawn_bones = pose
            .iter()
            .enumerate()
            .map(|(index, skinning)| {
                gla.skeleton
                    .get(index)
                    .map(|bone| multiply_3x4(skinning, &bone.base_pose))
                    .unwrap_or(*skinning)
            })
            .collect::<Vec<_>>();
        let mut driven = Vec::new();
        let mut soft_joints = Vec::new();
        let mut handles_by_bone = HashMap::<usize, RigidBodyHandle>::new();

        for spec in BODY_SPECS {
            let Some(spec_bone_index) = bone_index(gla, spec.name) else {
                continue;
            };
            let Some(bone_pose) = spawn_bones.get(spec_bone_index).copied() else {
                continue;
            };
            let scaled_bone_pose = scale_bone_translation(&bone_pose, model_scale);
            let world_matrix = multiply_3x4(&entity_matrix, &scaled_bone_pose);
            let world_pose = matrix_to_pose(&world_matrix);
            let body_builder = if kind == RagdollKind::Impulse && spec.name == "thoracic" {
                // Unlike Force Grip, impulse ragdolls are attached through the
                // chest itself. Keeping this one body kinematic follows the
                // authoritative player while every connected segment remains
                // a real dynamic rigid body.
                RigidBodyBuilder::kinematic_position_based().pose(world_pose)
            } else {
                RigidBodyBuilder::dynamic()
                    .pose(world_pose)
                    .linvel(velocity)
                    .linear_damping(if self.config.joint_model == 1 { 0.02 } else { 0.8 })
                    .angular_damping(if self.config.joint_model == 1 { 0.05 } else { 4.5 })
                    .ccd_enabled(self.config.ccd)
                    .can_sleep(self.config.sleeping)
                    .additional_solver_iterations(8)
            };
            let body = self.bodies.insert(body_builder);

            if let Some(spawned) = self.bodies.get(body) {
                self.previous_body_poses
                    .insert(body, spawned.position().clone());
            }

            let mut filter = Group::GROUP_1;
            if self.config.self_collision { filter |= Group::GROUP_2; }
            if kind == RagdollKind::Corpse { filter |= Group::GROUP_3; }
            let dynamic_groups = InteractionGroups::new(Group::GROUP_2, filter, InteractionTestMode::default());
            let collider = if let Some(end_name) = spec.end {
                let endpoint = bone_index(gla, end_name)
                    .and_then(|end_index| posed_bone_matrix(gla, pose, end_index))
                    .and_then(|end_pose| local_bone_endpoint(&bone_pose, &end_pose))
                    .unwrap_or([0.0, 0.0, spec.radius * 2.0]);
                let endpoint = Vector::new(endpoint[0], endpoint[1], endpoint[2])
                    * (JKA_TO_RAPIER * model_scale);
                ColliderBuilder::capsule_from_endpoints(
                    Vector::ZERO,
                    endpoint,
                    spec.radius * JKA_TO_RAPIER * model_scale,
                )
            } else {
                ColliderBuilder::ball(spec.radius * JKA_TO_RAPIER * model_scale)
            };
            self.colliders.insert_with_parent(
                collider
                    .mass((spec.mass_kg * model_scale.powi(3)).max(0.05))
                    .friction(0.9)
                    .restitution(0.0)
                    .collision_groups(dynamic_groups),
                body,
                &mut self.bodies,
            );
            handles_by_bone.insert(spec_bone_index, body);
            driven.push(DrivenBone {
                bone_index: spec_bone_index,
                body,
                // Use the same rigidization Rapier saw at spawn, but keep it
                // in unscaled model space. This makes the first-frame delta
                // exactly identity even when the original Ghoul2 pose had a
                // little interpolation shear.
                spawn_model_rigid: rigidify_affine(&bone_pose),
            });
        }

        // A living Force-grip ragdoll hangs from the cervical/neck point.
        // The anchor itself is kinematic and has no collider; server movement
        // drives only this point, while Rapier remains free to solve the rest
        // of the living body around it.
        let grip_anchor = if kind == RagdollKind::ForceGrip {
            let cervical_index = bone_index(gla, "cervical");
            let thoracic_index = bone_index(gla, "thoracic");
            match (cervical_index, thoracic_index) {
                (Some(cervical_index), Some(thoracic_index)) => {
                    let neck_pose = spawn_bones
                        .get(cervical_index)
                        .copied()
                        .ok_or_else(|| "Force-grip ragdoll missing cervical pose".to_owned())?;
                    let neck_model_position = [
                        neck_pose[0][3],
                        neck_pose[1][3],
                        neck_pose[2][3],
                    ];
                    let neck_scaled = [
                        neck_model_position[0] * model_scale,
                        neck_model_position[1] * model_scale,
                        neck_model_position[2] * model_scale,
                    ];
                    let neck_world_jka = transform_point(&entity_matrix, neck_scaled);
                    let neck_world = Vector::new(
                        neck_world_jka[0],
                        neck_world_jka[1],
                        neck_world_jka[2],
                    ) * JKA_TO_RAPIER;
                    let anchor_body = self.bodies.insert(
                        RigidBodyBuilder::kinematic_position_based()
                            .pose(Pose::from_parts(neck_world, Rotation::IDENTITY))
                            .build(),
                    );
                    let Some(&thoracic_body) = handles_by_bone.get(&thoracic_index) else {
                        self.remove_body(anchor_body);
                        return Err("Force-grip ragdoll has no thoracic rigid body".to_owned());
                    };
                    let Some(thoracic) = self.bodies.get(thoracic_body) else {
                        self.remove_body(anchor_body);
                        return Err("Force-grip ragdoll lost thoracic rigid body".to_owned());
                    };
                    let thoracic_anchor = inverse_pose_point(thoracic.position(), neck_world);
                    let joint = SphericalJointBuilder::new()
                        .local_frame1(Pose::from_parts(
                            Vector::new(0.0, 0.0, 0.0),
                            Rotation::IDENTITY,
                        ))
                        .local_frame2(Pose::from_parts(thoracic_anchor, Rotation::IDENTITY))
                        .contacts_enabled(false);
                    self.impulse_joints
                        .insert(anchor_body, thoracic_body, joint, true);
                    Some(GripAnchor {
                        body: anchor_body,
                        neck_model_position,
                    })
                }
                _ => None,
            }
        } else {
            None
        };

        let torso_anchor = if kind == RagdollKind::Impulse {
            bone_index(gla, "thoracic")
                .and_then(|thoracic_index| {
                    let body = handles_by_bone.get(&thoracic_index).copied()?;
                    let torso_model_rigid = spawn_bones.get(thoracic_index).copied()?;
                    Some(TorsoAnchor { body, torso_model_rigid: rigidify_affine(&torso_model_rigid) })
                })
        } else {
            None
        };

        // Join every driven segment to its nearest driven skeleton ancestor.
        // This follows the actual model hierarchy instead of assuming all custom
        // player GLAs have identical intermediary twist bones.
        for child in &driven {
            let Some(parent_index) = nearest_driven_parent(gla, child.bone_index, &handles_by_bone) else {
                continue;
            };
            let Some(&parent_handle) = handles_by_bone.get(&parent_index) else {
                continue;
            };
            let Some(child_pose) = spawn_bones.get(child.bone_index).copied() else {
                continue;
            };
            let child_pose = scale_bone_translation(&child_pose, model_scale);
            let child_world = multiply_3x4(&entity_matrix, &child_pose);
            let anchor_world = Vector::new(
                child_world[0][3],
                child_world[1][3],
                child_world[2][3],
            ) * JKA_TO_RAPIER;
            let Some(parent_body) = self.bodies.get(parent_handle) else {
                continue;
            };
            let Some(child_body) = self.bodies.get(child.body) else {
                continue;
            };
            let parent_anchor = inverse_pose_point(parent_body.position(), anchor_world);
            let child_anchor = inverse_pose_point(child_body.position(), anchor_world);

            let bone_name = gla.skeleton
                .get(child.bone_index)
                .map(|bone| bone.name.as_str())
                .unwrap_or("");
            if self.config.joint_model == 0 {
                // Exact legacy path: hard limits centered on the handoff pose.
                let frame = parent_body.position().rotation.inverse() * child_body.position().rotation;
                let limits = joint_limits_for_bone(bone_name);
                let joint = SphericalJointBuilder::new()
                    .local_frame1(Pose::from_parts(parent_anchor, frame))
                    .local_frame2(Pose::from_parts(child_anchor, Rotation::IDENTITY))
                    .limits(JointAxis::AngX, limits[0])
                    .limits(JointAxis::AngY, limits[1])
                    .limits(JointAxis::AngZ, limits[2])
                    .contacts_enabled(false);
                self.impulse_joints.insert(parent_handle, child.body, joint, true);
                continue;
            }

            // Never change the stable Rapier anchors / hard stops from Classic.
            // The Blender solver also uses a separate anatomical joint frame;
            // it must not be silently substituted for the Rapier joint frame.
            let frame = parent_body.position().rotation.inverse() * child_body.position().rotation;
            let limits = joint_limits_for_bone(bone_name);
            let joint = SphericalJointBuilder::new()
                .local_frame1(Pose::from_parts(parent_anchor, frame))
                .local_frame2(Pose::from_parts(child_anchor, Rotation::IDENTITY))
                .limits(JointAxis::AngX, limits[0])
                .limits(JointAxis::AngY, limits[1])
                .limits(JointAxis::AngZ, limits[2])
                .contacts_enabled(false);
            self.impulse_joints.insert(parent_handle, child.body, joint, true);

            // Mirror model.py's anatomical frames and per-role preset angles.
            // A rig lacking reliable bone directions gets Classic constraints
            // on that joint; never invent an axis from the death animation.
            if let Some((frame_p, frame_c, ranges)) = anatomical_soft_joint(
                gla, parent_index, child.bone_index, bone_name,
            ) {
                let effective_inertia = subtree_joint_inertia(
                    gla, child.bone_index, model_scale, &driven, &self.bodies,
                );
                soft_joints.push(SoftJoint {
                    parent: parent_handle, child: child.body,
                    frame_p, frame_c, ranges, effective_inertia,
                });
            }
        }

        Ok(RagdollInstance {
            model_key: model_key.to_owned(),
            model_scale,
            generation,
            kind,
            grip_anchor,
            torso_anchor,
            spawned_at: current_time,
            lifetime_slept: false,
            last_authoritative_origin: origin,
            last_applied_pose: spawn_pose.clone(),
            spawn_pose,
            driven,
            soft_joints,
        })
    }

    /// Drive the kinematic Force-grip anchor from the authoritative entity
    /// transform. The stored cervical point is frozen at handoff so normal live
    /// animation can no longer pull the visual body around; only the server's
    /// victim motion moves the neck anchor.
    fn update_grip_anchor(
        &mut self,
        entity: u16,
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        model_scale: f32,
    ) {
        let Some(instance) = self.instances.get(&entity) else {
            return;
        };
        let Some(anchor) = instance.grip_anchor else {
            return;
        };
        let neck = [
            anchor.neck_model_position[0] * model_scale,
            anchor.neck_model_position[1] * model_scale,
            anchor.neck_model_position[2] * model_scale,
        ];
        let world_jka = transform_point(&entity_matrix(axis, origin), neck);
        let target = Vector::new(world_jka[0], world_jka[1], world_jka[2]) * JKA_TO_RAPIER;
        if let Some(body) = self.bodies.get_mut(anchor.body) {
            body.set_next_kinematic_translation(target);
        }
    }

    /// Drive the thoracic kinematic body from the authoritative living-player
    /// transform. Translation and orientation both follow the server-owned
    /// player; connected head/arms/spine/pelvis/legs remain dynamic.
    fn update_torso_anchor(
        &mut self,
        entity: u16,
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        model_scale: f32,
    ) {
        let Some(instance) = self.instances.get(&entity) else {
            return;
        };
        let Some(anchor) = instance.torso_anchor else {
            return;
        };
        let torso_model = scale_bone_translation(&anchor.torso_model_rigid, model_scale);
        let world = multiply_3x4(&entity_matrix(axis, origin), &torso_model);
        let target = matrix_to_pose(&world);
        if let Some(body) = self.bodies.get_mut(anchor.body) {
            body.set_next_kinematic_position(target);
        }
    }

    /// Keep the local articulated solution attached to the authoritative JKA
    /// corpse trajectory. This intentionally applies translation only: server
    /// yaw/pitch changes should not continuously re-pose a solved ragdoll, but
    /// server displacement must remain visible.
    fn reconcile_authoritative_translation(&mut self, entity: u16, origin: [f32; 3]) {
        let debug = self.config.debug;
        let (instances, bodies, previous_body_poses) = (
            &mut self.instances,
            &mut self.bodies,
            &mut self.previous_body_poses,
        );
        let Some(instance) = instances.get_mut(&entity) else {
            return;
        };
        let delta_jka = Vector::new(
            origin[0] - instance.last_authoritative_origin[0],
            origin[1] - instance.last_authoritative_origin[1],
            origin[2] - instance.last_authoritative_origin[2],
        );
        instance.last_authoritative_origin = origin;

        if delta_jka.length_squared() <= 1.0e-8 {
            return;
        }
        let delta = delta_jka * JKA_TO_RAPIER;

        // Move both ends of the render interpolation interval by exactly the
        // same amount. Otherwise a network-driven push would visually lag by
        // one physics sample even though the authoritative entity is already
        // at its new position. set_translation(..., true) wakes sleeping bodies
        // so contacts are re-solved at the destination.
        for driven in &instance.driven {
            if let Some(body) = bodies.get_mut(driven.body) {
                let translation = body.translation() + delta;
                body.set_translation(translation, true);
            }
            if let Some(previous) = previous_body_poses.get_mut(&driven.body) {
                previous.translation += delta;
            }
        }

        if debug {
            println!(
                "RAPIER AUTH SYNC: entity={} delta=[{:.2},{:.2},{:.2}]",
                entity, delta_jka.x, delta_jka.y, delta_jka.z
            );
        }
    }

    fn capture_previous_poses(&mut self) {
        for instance in self.instances.values() {
            for driven in &instance.driven {
                if let Some(body) = self.bodies.get(driven.body) {
                    self.previous_body_poses
                        .insert(driven.body, body.position().clone());
                }
            }
        }
        for detached in self.detached_limbs.values() {
            if let Some(body) = self.bodies.get(detached.body) {
                self.previous_body_poses
                    .insert(detached.body, body.position().clone());
            }
        }
        for debris in &self.debris {
            if let Some(body) = self.bodies.get(debris.body) {
                self.previous_body_poses.insert(debris.body, body.position().clone());
            }
        }
        for glass in &self.glass {
            if let Some(handle) = glass.body {
                if let Some(body) = self.bodies.get(handle) {
                    self.previous_body_poses.insert(handle, body.position().clone());
                }
            }
        }
    }

    fn interpolation_alpha(&self) -> f32 {
        let step_dt = 1.0 / self.config.hz.max(1) as f32;
        (self.accumulator / step_dt).clamp(0.0, 1.0)
    }

    fn sleep_expired(&mut self, current_time: i32) {
        if self.config.lifetime_seconds > 0.0 {
            let lifetime_ms = (self.config.lifetime_seconds * 1000.0) as i32;
            for instance in self.instances.values_mut() {
                if instance.lifetime_slept || current_time.saturating_sub(instance.spawned_at) < lifetime_ms {
                    continue;
                }
                // Apply the lifetime sleep once; later player contact must be
                // able to wake the corpse and keep it moving on following frames.
                instance.lifetime_slept = true;
                for driven in &instance.driven {
                    if let Some(body) = self.bodies.get_mut(driven.body) {
                        body.sleep();
                    }
                }
            }
        }

        let detached_lifetime_ms = (self.config.detached_limb_lifetime_seconds * 1000.0) as i32;
        let expired = self
            .detached_limbs
            .iter()
            .filter_map(|(&entity, limb)| {
                (detached_lifetime_ms > 0
                    && current_time.saturating_sub(limb.spawned_at) >= detached_lifetime_ms)
                    .then_some(entity)
            })
            .collect::<Vec<_>>();
        for entity in expired {
            self.remove_detached_limb(entity);
        }

        // CCD matters during the initial high-speed flight, not after a limb has
        // settled. Turning it off below ~120 JKA u/s saves narrow/TOI work.
        let ccd_threshold_sq = (120.0 * JKA_TO_RAPIER).powi(2);
        for limb in self.detached_limbs.values() {
            if let Some(body) = self.bodies.get_mut(limb.body) {
                let fast = body.linvel().length_squared() > ccd_threshold_sq;
                body.enable_ccd(self.config.ccd && fast);
            }
        }
    }

    /// Create exactly one rigid body for a server-authored detached Ghoul2 part.
    /// Repeated snapshot presentation is idempotent; a reused entity number with
    /// a new generation tears down the previous body first.
    pub(crate) fn ensure_detached_limb(
        &mut self,
        spawn: DetachedLimbSpawn,
        current_time: i32,
    ) -> bool {
        if !self.detached_physics_active() {
            return false;
        }
        if self
            .detached_limbs
            .get(&spawn.entity)
            .is_some_and(|limb| limb.generation == spawn.generation)
        {
            return true;
        }
        self.remove_detached_limb(spawn.entity);

        let linear_velocity = Vector::new(
            spawn.linear_velocity[0],
            spawn.linear_velocity[1],
            spawn.linear_velocity[2],
        ) * JKA_TO_RAPIER;
        let angular_velocity = Vector::new(
            spawn.angular_velocity[0],
            spawn.angular_velocity[1],
            spawn.angular_velocity[2],
        );
        let body = self.bodies.insert(
            RigidBodyBuilder::dynamic()
                .pose(matrix_to_pose(&spawn.body_world))
                .linvel(linear_velocity)
                .angvel(angular_velocity)
                .linear_damping(0.45)
                .angular_damping(1.6)
                .ccd_enabled(self.config.ccd && linear_velocity.length_squared() > (120.0 * JKA_TO_RAPIER).powi(2))
                .can_sleep(self.config.sleeping)
                .additional_solver_iterations(4),
        );
        if let Some(rigid) = self.bodies.get(body) {
            self.previous_body_poses.insert(body, rigid.position().clone());
        }
        let groups = InteractionGroups::new(
            Group::GROUP_2,
            Group::GROUP_1,
            InteractionTestMode::default(),
        );
        let collider = if let Some((a, b)) = spawn.capsule_local {
            ColliderBuilder::capsule_from_endpoints(
                Vector::new(a[0], a[1], a[2]) * JKA_TO_RAPIER,
                Vector::new(b[0], b[1], b[2]) * JKA_TO_RAPIER,
                spawn.radius.max(0.25) * JKA_TO_RAPIER,
            )
        } else {
            ColliderBuilder::ball(spawn.radius.max(0.25) * JKA_TO_RAPIER)
        };
        self.colliders.insert_with_parent(
            collider
                .mass(spawn.mass_kg.max(0.05))
                .friction(0.9)
                .restitution(0.05)
                .collision_groups(groups),
            body,
            &mut self.bodies,
        );
        self.detached_limbs.insert(
            spawn.entity,
            DetachedLimbBody {
                generation: spawn.generation,
                body,
                spawned_at: current_time,
            },
        );
        self.trim_detached_to_budget();
        true
    }

    /// Interpolated rigid transform for rendering at display FPS while physics
    /// remains fixed-step at r_physicsHz.
    pub(crate) fn detached_limb_matrix(
        &self,
        entity: u16,
        generation: DetachedLimbGeneration,
    ) -> Option<Matrix3x4> {
        let limb = self.detached_limbs.get(&entity)?;
        if limb.generation != generation {
            return None;
        }
        let current = self.bodies.get(limb.body)?.position();
        let previous = self.previous_body_poses.get(&limb.body).unwrap_or(current);
        Some(pose_to_matrix(&interpolate_pose(
            previous,
            current,
            self.interpolation_alpha(),
        )))
    }

    /// Prefer the live Rapier segment velocity when a sever occurs after the
    /// source has already entered a ragdoll. This makes a flying corpse's limb
    /// continue naturally instead of restarting from the network trajectory.
    pub(crate) fn ragdoll_bone_velocity(
        &self,
        entity: u16,
        gla: &GlaAnimation,
        bone_name: &str,
    ) -> Option<[f32; 3]> {
        let instance = self.instances.get(&entity)?;
        let bone = bone_index(gla, bone_name)?;
        let driven = instance
            .driven
            .iter()
            .find(|driven| driven.bone_index == bone)
            .or_else(|| {
                let mut parent = jka_assets::skeleton::mesh_pose_parent(&gla.skeleton, bone);
                while let Some(index) = parent {
                    if let Some(driven) = instance.driven.iter().find(|driven| driven.bone_index == index) {
                        return Some(driven);
                    }
                    parent = jka_assets::skeleton::mesh_pose_parent(&gla.skeleton, index);
                }
                None
            })?;
        let v = self.bodies.get(driven.body)?.linvel() * RAPIER_TO_JKA;
        Some([v.x, v.y, v.z])
    }

    pub(crate) fn retain_detached_limbs(&mut self, live: &HashSet<u16>) {
        let stale = self
            .detached_limbs
            .keys()
            .copied()
            .filter(|entity| !live.contains(entity))
            .collect::<Vec<_>>();
        for entity in stale {
            self.remove_detached_limb(entity);
        }
    }

    fn oldest_detached_limb(&self) -> Option<u16> {
        self.detached_limbs
            .iter()
            .min_by_key(|(_, limb)| limb.spawned_at)
            .map(|(&entity, _)| entity)
    }

    fn trim_detached_to_budget(&mut self) {
        while self.detached_limbs.len() > self.config.max_detached_limbs as usize {
            let Some(oldest) = self.oldest_detached_limb() else { break };
            self.remove_detached_limb(oldest);
        }
    }

    fn remove_detached_limb(&mut self, entity: u16) {
        let Some(limb) = self.detached_limbs.remove(&entity) else { return };
        self.remove_body(limb.body);
    }

    fn clear_detached_limbs(&mut self) {
        let entities = self.detached_limbs.keys().copied().collect::<Vec<_>>();
        for entity in entities {
            self.remove_detached_limb(entity);
        }
    }

    fn oldest_instance(&self) -> Option<u16> {
        self.instances
            .iter()
            .min_by_key(|(_, instance)| instance.spawned_at)
            .map(|(&entity, _)| entity)
    }

    fn trim_to_budget(&mut self) {
        while self.instances.len() > self.config.max_ragdolls as usize {
            let Some(oldest) = self.oldest_instance() else {
                break;
            };
            self.retire_instance(oldest);
        }
    }

    fn clear_instances(&mut self) {
        let entities = self.instances.keys().copied().collect::<Vec<_>>();
        for entity in entities {
            self.remove_instance(entity);
        }
        self.retired.clear();
        self.living_recoveries.clear();
    }

    /// OpenJK `CG_BodyQueueCopy` duplicates the source Ghoul2 corpse at its
    /// already-solved pose. Move the corresponding Rapier articulation to the
    /// destination entity instead of spawning a second ragdoll from the death
    /// animation when ET_BODY first appears.
    pub(crate) fn body_queue_copy(&mut self, source: u16, body: u16) {
        if source == body {
            return;
        }

        // Body-queue slots are reused; discard any physics left by the previous
        // occupant before transferring the new corpse into that slot.
        self.remove_instance(body);
        self.retired.remove(&body);
        self.living_recoveries.remove(&body);
        self.debug_candidates.remove(&body);

        let Some(mut instance) = self.instances.remove(&source) else {
            return;
        };
        if instance.kind != RagdollKind::Corpse {
            // A body copy is only meaningful for an already-dead articulation.
            // Put any other presentation takeover back untouched.
            self.instances.insert(source, instance);
            return;
        }
        instance.generation.body_queue_time = BODY_QUEUE_PENDING_TIME;
        self.instances.insert(body, instance);
        self.debug_candidates.remove(&source);
    }

    /// Reliable `kg2` removes the Ghoul2 instance immediately; mirror that for
    /// any physics state bolted to the same presentation entity.
    pub(crate) fn remove_entity(&mut self, entity: u16) {
        self.remove_instance(entity);
        self.remove_detached_limb(entity);
        self.retired.remove(&entity);
        self.living_recoveries.remove(&entity);
    }

    fn retire_instance(&mut self, entity: u16) {
        let Some(instance) = self.instances.get(&entity) else {
            return;
        };
        let retired = (instance.model_key.clone(), instance.generation);
        self.remove_instance(entity);
        self.retired.insert(entity, retired);
    }

    fn remove_instance(&mut self, entity: u16) {
        let Some(instance) = self.instances.remove(&entity) else {
            return;
        };
        for driven in instance.driven {
            self.remove_body(driven.body);
        }
        if let Some(anchor) = instance.grip_anchor {
            self.remove_body(anchor.body);
        }
    }

    fn remove_body(&mut self, handle: RigidBodyHandle) {
        self.previous_body_poses.remove(&handle);
        let _ = self.bodies.remove(
            handle,
            &mut self.islands,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            true,
        );
    }

    /// Accept an EV_DEBRIS spray into the same rigid-body/CCD solver as corpses
    /// and severed limbs. Returns false when the stock ballistic fallback is
    /// required (physics off or static collision not ready yet).
    pub(crate) fn spawn_debris(&mut self, specs: &[ChunkSpec], now: i32) -> bool {
        if !self.debris_active() || self.config.max_debris == 0 {
            return false;
        }
        // Keep the authoritative spawn distribution/materials. Only the
        // fragment motion and retention are optional client presentation.
        for spec in specs {
            if !spec.origin.iter().chain(spec.velocity.iter()).chain(spec.angles.iter())
                .chain(spec.angle_velocity.iter()).all(|v| v.is_finite())
                || !spec.scale.is_finite() || spec.scale <= 0.0
            {
                continue;
            }
            let basis = crate::cgame::weapon_fx::rapier_debris_axis(spec.angles);
            let rotation = Rotation::from_mat3(&Matrix::from_cols(
                Vector::from_array(basis[0]),
                Vector::from_array(basis[1]),
                Vector::from_array(basis[2]),
            )).normalize();
            let position = Vector::from_array(spec.origin) * JKA_TO_RAPIER;
            let velocity = Vector::from_array(spec.velocity) * JKA_TO_RAPIER;
            let angular_velocity = Vector::new(
                spec.angle_velocity[0].to_radians(),
                spec.angle_velocity[1].to_radians(),
                spec.angle_velocity[2].to_radians(),
            );
            let body = self.bodies.insert(
                RigidBodyBuilder::dynamic()
                    .pose(Pose::from_parts(position, rotation))
                    .linvel(velocity)
                    .angvel(angular_velocity)
                    .linear_damping(0.30)
                    .angular_damping(0.9)
                    .ccd_enabled(self.config.ccd)
                    .can_sleep(self.config.sleeping),
            );
            if let Some(rigid) = self.bodies.get(body) {
                self.previous_body_poses.insert(body, rigid.position().clone());
            }
            // The collision shape is deliberately a cheap proxy. MD3 fragments
            // retain their original rendered geometry, without expensive hull
            // extraction during a burst. GROUP_2 can contact the world, while
            // enabling debris-debris contacts without forcing ragdoll coupling.
            let radius = (3.0 * spec.scale).clamp(0.5, 12.0) * JKA_TO_RAPIER;
            let groups = InteractionGroups::new(
                Group::GROUP_2, Group::GROUP_1 | Group::GROUP_2,
                InteractionTestMode::default(),
            );
            self.colliders.insert_with_parent(
                ColliderBuilder::ball(radius)
                    .mass(0.3)
                    .friction(0.8)
                    .restitution(spec.bounce_factor.clamp(0.0, 1.0))
                    .collision_groups(groups),
                body,
                &mut self.bodies,
            );
            self.debris.push(DebrisBody {
                body,
                qpath: spec.qpath.clone(),
                scale: spec.scale,
                expires_at: now.saturating_add((self.config.debris_lifetime_seconds * 1000.0) as i32),
            });
            self.trim_debris_to_budget();
        }
        true
    }

    /// Returns the prefix actually accepted; unaccepted quads remain legacy
    /// FX polys, including when physics/collision is unavailable.
    pub(crate) fn spawn_glass(&mut self, specs: &[ShardSpec], now: i32) -> usize {
        if !self.debris_active() || self.config.max_debris == 0 {
            return 0;
        }
        let count = specs.len().min(self.config.max_debris as usize);
        // A prefix count is unambiguous only if every accepted spec is valid.
        if specs.iter().take(count).any(|spec| {
            spec.positions.iter().flatten().chain(spec.velocity.iter())
                .chain(spec.rotation.iter()).any(|v| !v.is_finite())
                || !spec.bounce.is_finite() || !spec.accel_z.is_finite()
        }) {
            return 0;
        }
        for spec in specs.iter().take(count) {
            let centre = std::array::from_fn(|axis| {
                spec.positions.iter().map(|p| p[axis]).sum::<f32>() * 0.25
            });
            self.glass.push(GlassBody {
                body: None,
                spec: spec.clone(),
                centre,
                born_at: now,
                activate_at: now.saturating_add(spec.delay_ms.max(0)),
                expires_at: now.saturating_add((self.config.debris_lifetime_seconds * 1000.0) as i32),
            });
        }
        self.trim_debris_to_budget();
        count
    }

    fn activate_glass(&mut self, now: i32) {
        for glass in &mut self.glass {
            if glass.body.is_some() || now < glass.activate_at {
                continue;
            }
            let min = std::array::from_fn::<_, 3, _>(|axis| glass.spec.positions.iter()
                .map(|point| point[axis]).fold(f32::INFINITY, f32::min));
            let max = std::array::from_fn::<_, 3, _>(|axis| glass.spec.positions.iter()
                .map(|point| point[axis]).fold(f32::NEG_INFINITY, f32::max));
            // Axis-aligned at spawn: each pane is planar, with minimal thickness
            // along its normal. The original quad remains the rendered surface.
            let half = std::array::from_fn::<_, 3, _>(|axis| {
                ((max[axis] - min[axis]) * 0.5).max(0.45).min(128.0) * JKA_TO_RAPIER
            });
            let body = self.bodies.insert(
                RigidBodyBuilder::dynamic()
                    .translation(Vector::from_array(glass.centre) * JKA_TO_RAPIER)
                    .linvel(Vector::from_array(glass.spec.velocity) * JKA_TO_RAPIER)
                    .angvel(Vector::new(glass.spec.rotation[0].to_radians(), glass.spec.rotation[1].to_radians(), 0.0))
                    .gravity_scale((-glass.spec.accel_z / JKA_GRAVITY).clamp(0.1, 2.0))
                    .linear_damping(0.22)
                    .angular_damping(0.55)
                    .ccd_enabled(self.config.ccd)
                    .can_sleep(self.config.sleeping),
            );
            if let Some(rigid) = self.bodies.get(body) {
                self.previous_body_poses.insert(body, rigid.position().clone());
            }
            self.colliders.insert_with_parent(
                ColliderBuilder::cuboid(half[0], half[1], half[2])
                    .mass(0.12)
                    .friction(0.65)
                    .restitution(glass.spec.bounce.clamp(0.0, 1.0))
                    .collision_groups(InteractionGroups::new(
                        Group::GROUP_2, Group::GROUP_1 | Group::GROUP_2,
                        InteractionTestMode::default(),
                    )),
                body,
                &mut self.bodies,
            );
            glass.body = Some(body);
        }
    }

    /// Tessellated UVs/transparency/shader are preserved. Rapier supplies only
    /// an interpolated rigid transform around the original shard centre.
    pub(crate) fn glass_draws(&self, now: i32) -> Vec<FxDraw> {
        self.glass.iter().filter_map(|glass| {
            if now < glass.born_at || now >= glass.expires_at { return None; }
            let duration = (glass.expires_at - glass.born_at).max(1);
            let life = (now - glass.born_at) as f32 / duration as f32;
            let alpha = if life < 0.85 { 0.15 } else { (0.15 - (life - 0.85)).max(0.0) };
            if alpha <= 0.0 { return None; }
            let positions = if let Some(handle) = glass.body {
                let body = self.bodies.get(handle)?;
                let pose = body.position();
                let previous = self.previous_body_poses.get(&handle).unwrap_or(pose);
                let display = interpolate_pose(previous, pose, self.interpolation_alpha());
                let centre = display.translation * RAPIER_TO_JKA;
                glass.spec.positions.map(|point| {
                    let relative = Vector::from_array(std::array::from_fn(|axis| point[axis] - glass.centre[axis]));
                    let p = centre + display.rotation * relative;
                    [p.x, p.y, p.z]
                })
            } else {
                glass.spec.positions
            };
            Some(FxDraw::Quad {
                positions, uvs: glass.spec.uvs,
                rgba: [255, 255, 255, (alpha * 255.0) as u8],
                shader: "gfx/misc/test_crackle".to_owned(),
            })
        }).collect()
    }

    fn expire_glass(&mut self, now: i32) {
        let mut i = 0;
        while i < self.glass.len() {
            if self.glass[i].expires_at <= now {
                let glass = self.glass.remove(i);
                if let Some(handle) = glass.body { self.remove_body(handle); }
            } else { i += 1; }
        }
    }

    fn clear_glass(&mut self) {
        let old = std::mem::take(&mut self.glass);
        for glass in old {
            if let Some(handle) = glass.body { self.remove_body(handle); }
        }
    }

    pub(crate) fn debris_models(&self, now: i32) -> Vec<ChunkModel> {
        const FADE_MS: i32 = 2000;
        self.debris.iter().filter_map(|fragment| {
            let rigid = self.bodies.get(fragment.body)?;
            let pose = rigid.position();
            let previous = self.previous_body_poses.get(&fragment.body).unwrap_or(pose);
            let display = interpolate_pose(previous, pose, self.interpolation_alpha());
            let basis = [Vector::X, Vector::Y, Vector::Z].map(|axis| {
                let v = display.rotation * axis * fragment.scale;
                [v.x, v.y, v.z]
            });
            let p = display.translation * RAPIER_TO_JKA;
            Some(ChunkModel {
                qpath: fragment.qpath.clone(),
                origin: [p.x, p.y, p.z],
                axis: basis,
                alpha: ((fragment.expires_at.saturating_sub(now)) as f32 / FADE_MS as f32).clamp(0.0, 1.0),
            })
        }).collect()
    }

    fn expire_debris(&mut self, now: i32) {
        let mut index = 0;
        while index < self.debris.len() {
            if self.debris[index].expires_at <= now {
                let fragment = self.debris.remove(index);
                self.remove_body(fragment.body);
            } else {
                index += 1;
            }
        }
    }

    fn trim_debris_to_budget(&mut self) {
        while self.debris.len() + self.glass.len() > self.config.max_debris as usize {
            // Oldest visual fragments give way to new sprays; both visual
            // formats use the same rigid-body limit, not separate budgets.
            if !self.debris.is_empty() && (self.glass.is_empty()
                || self.debris[0].expires_at <= self.glass[0].expires_at)
            {
                let fragment = self.debris.remove(0);
                self.remove_body(fragment.body);
            } else {
                let glass = self.glass.remove(0);
                if let Some(handle) = glass.body { self.remove_body(handle); }
            }
        }
    }

    fn clear_debris(&mut self) {
        let debris = std::mem::take(&mut self.debris);
        for fragment in debris {
            self.remove_body(fragment.body);
        }
    }

    fn reset_world(&mut self) {
        self.pipeline = PhysicsPipeline::new();
        self.integration = IntegrationParameters::default();
        self.islands = IslandManager::new();
        self.broad_phase = BroadPhaseBvh::new();
        self.narrow_phase = NarrowPhase::new();
        self.bodies = RigidBodySet::new();
        self.colliders = ColliderSet::new();
        self.impulse_joints = ImpulseJointSet::new();
        self.multibody_joints = MultibodyJointSet::new();
        self.ccd_solver = CCDSolver::new();
        self.static_world_ready = false;
        self.previous_body_poses.clear();
        self.instances.clear();
        self.player_colliders.clear();
        self.detached_limbs.clear();
        self.debris.clear();
        self.glass.clear();
        self.living_recoveries.clear();
        self.retired.clear();
        self.last_time = None;
        self.accumulator = 0.0;
        self.last_stats_print = None;
        self.debug_candidates.clear();
    }
}

fn corpse_generation(entity: &PresentedEntity) -> CorpseGeneration {
    CorpseGeneration {
        client_num: entity
            .state
            .field_i32("clientNum")
            .unwrap_or(i32::from(entity.number)),
        // A live NPC/player keeps the same entity number throughout one death
        // while the server updates trBase/trTime/trDelta as it slides/falls.
        // Those fields must NOT be treated as a new ragdoll generation. ET_BODY
        // queue slots, on the other hand, can be reused for later corpses, and
        // CopyToBodyQue stamps pos.trTime for moving bodies at the copy point.
        body_queue_time: if entity.entity_type == crate::cgame::ET_BODY {
            entity.state.field_i32("pos.trTime").unwrap_or(0)
        } else {
            0
        },
    }
}

fn joint_limits_for_bone(name: &str) -> [[f32; 2]; 3] {
    fn range(degrees: f32) -> [f32; 2] {
        let radians = degrees.to_radians();
        [-radians, radians]
    }

    // These are intentionally broad Broadsword-style safety limits rather
    // than a pose driver. Their job is to stop 360-degree elbows/spines and
    // other solver-only contortions while preserving the exact death pose as
    // the center of each joint's range.
    let degrees = match name.to_ascii_lowercase().as_str() {
        "lower_lumbar" | "upper_lumbar" | "thoracic" => [25.0, 20.0, 25.0],
        "cranium" => [50.0, 40.0, 55.0],
        "rhumerus" | "lhumerus" => [90.0, 90.0, 90.0],
        "rradius" | "lradius" => [75.0, 75.0, 75.0],
        "rhand" | "lhand" => [45.0, 45.0, 45.0],
        "rfemuryz" | "lfemuryz" => [70.0, 65.0, 70.0],
        "rtibia" | "ltibia" => [70.0, 70.0, 70.0],
        "rtalus" | "ltalus" | "rtarsal" | "ltarsal" => [40.0, 35.0, 40.0],
        _ => [60.0, 60.0, 60.0],
    };
    [range(degrees[0]), range(degrees[1]), range(degrees[2])]
}

/// Presets from better_ragdoll/presets.py. Bone names are the JKA/JK2
/// Ghoul2 names. Each triple is (low, high, neutral) in degrees.
fn soft_joint_preset(name: &str) -> Option<([[f32; 3]; 3], bool, bool)> {
    let name = name.to_ascii_lowercase();
    let (angles, down, parent_direction) = match name.as_str() {
        "lower_lumbar" | "upper_lumbar" | "thoracic" =>
            ([[-20.,35.,4.],[-20.,20.,0.],[-20.,20.,0.]],false,false),
        "cranium" => ([[-30.,35.,10.],[-20.,20.,0.],[-35.,35.,0.]],false,false),
        "rhumerus" | "lhumerus" =>
            ([[-50.,170.,38.],[-30.,165.,42.],[-70.,70.,10.]],true,false),
        "rradius" | "lradius" =>
            ([[0.,145.,65.],[-4.,4.,0.],[-80.,80.,0.]],false,true),
        "rhand" | "lhand" =>
            ([[-70.,70.,0.],[-20.,30.,5.],[-8.,8.,0.]],false,false),
        "rfemuryz" | "lfemuryz" =>
            ([[-20.,125.,48.],[-20.,45.,10.],[-35.,35.,5.]],true,false),
        "rtibia" | "ltibia" =>
            ([[0.,145.,50.],[-4.,4.,0.],[-8.,8.,0.]],false,true),
        "rtalus" | "ltalus" =>
            ([[-50.,20.,-20.],[-15.,15.,0.],[-20.,20.,0.]],false,false),
        // The Blender model merges toes with feet; they keep Classic joints.
        _ => return None,
    };
    let mut ranges: [[f32; 3]; 3] = angles;
    for channel in 0..3 {
        for term in 0..3 { ranges[channel][term] = ranges[channel][term].to_radians(); }
    }
    // Source model._limits mirrors abduction and axial twist, NOT flexion.
    if name.starts_with('r') {
        for range in &mut ranges[1..3] {
            let [lo, hi, mid] = *range;
            *range = [-hi, -lo, -mid];
        }
    }
    Some((ranges, down, parent_direction))
}

fn bind_bone_point(gla: &GlaAnimation, bone: usize) -> Option<Vector> {
    let m = &gla.skeleton.get(bone)?.base_pose;
    Some(Vector::new(m[0][3], m[1][3], m[2][3]))
}

/// Port of model.py's zero-direction, flex-direction, and frame construction.
/// JKA bones don't consistently point their local +Y toward the tail, so the
/// already-used Ghoul2 segment endpoints provide the equivalent rest vectors.
fn anatomical_soft_joint(
    gla: &GlaAnimation, parent: usize, child: usize, name: &str,
) -> Option<(Rotation, Rotation, [[f32; 3]; 3])> {
    let (ranges, down, continue_parent) = soft_joint_preset(name)?;
    let child_head = bind_bone_point(gla, child)?;
    let end_name = BODY_SPECS.iter().find(|spec| spec.name.eq_ignore_ascii_case(name))?.end;
    let child_bind = matrix_to_pose(&gla.skeleton[child].base_pose).rotation;
    let parent_bind = matrix_to_pose(&gla.skeleton[parent].base_pose).rotation;
    let rest_direction = end_name
        .and_then(|end| bone_index(gla, end))
        .and_then(|end| bind_bone_point(gla, end))
        .map(|tail| (tail - child_head).normalize_or_zero())
        .filter(|d| d.length_squared() > 0.25)
        .unwrap_or(child_bind * Vector::Y);
    if rest_direction.length_squared() < 0.25 { return None; }
    let y = if down { -Vector::Z }
        else if continue_parent {
            (child_head - bind_bone_point(gla, parent)?).normalize_or_zero()
        } else { rest_direction };
    if y.length_squared() < 0.25 { return None; }
    let right_hip = bone_index(gla, "rfemurYZ").and_then(|i| bind_bone_point(gla, i));
    let left_hip = bone_index(gla, "lfemurYZ").and_then(|i| bind_bone_point(gla, i));
    let root = bone_index(gla, "pelvis").and_then(|i| bind_bone_point(gla, i));
    let head = bone_index(gla, "cranium").and_then(|i| bind_bone_point(gla, i));
    let forward = match (right_hip, left_hip, root, head) {
        (Some(r), Some(l), Some(p), Some(h)) => {
            let direction = (r - l).cross(h - p).normalize_or_zero();
            if direction.length_squared() > 0.25 { direction } else { Vector::X }
        }
        _ => Vector::X,
    };
    let role = name.to_ascii_lowercase();
    let flex_dir = if role == "rtibia" || role == "ltibia" { -forward }
        else if role == "rtalus" || role == "ltalus" { Vector::Z }
        else { forward };
    let mut x = y.cross(flex_dir);
    if x.length_squared() < 0.0625 {
        let alt = if role == "rtalus" || role == "ltalus" { forward } else { Vector::Z };
        x = y.cross(alt);
    }
    x = x.normalize_or_zero();
    if x.length_squared() < 0.25 { return None; }
    let z = x.cross(y).normalize_or_zero();
    let frame_world = Rotation::from_mat3(&Mat3::from_cols(x, y, z));
    let child_world = Rotation::from_rotation_arc(y, rest_direction) * frame_world;
    Some((parent_bind.inverse() * frame_world,
          child_bind.inverse() * child_world, ranges))
}

/// Equivalent of model.py joint i_eff: all distal body masses and their
/// rotational inertia around the joint anchor (Rapier bodies are SI-scaled).
fn subtree_joint_inertia(
    gla: &GlaAnimation, child: usize, model_scale: f32,
    driven: &[DrivenBone], bodies: &RigidBodySet,
) -> f32 {
    // Blender model.py measures all subtree offsets in the REST pose,
    // never from the animation pose sampled at death.
    let Some(anchor) = bind_bone_point(gla, child) else { return 1.0e-6; };
    let to_meters = model_scale * JKA_TO_RAPIER;
    let mut moment = 0.0;
    for bone in driven {
        let mut current = Some(bone.bone_index);
        let mut distal = false;
        while let Some(index) = current {
            if index == child { distal = true; break; }
            current = jka_assets::skeleton::mesh_pose_parent(&gla.skeleton, index);
        }
        if !distal { continue; }
        let Some(body) = bodies.get(bone.body) else { continue; };
        let mass = body.mass();
        let Some(head) = bind_bone_point(gla, bone.bone_index) else { continue; };
        let end = BODY_SPECS.iter().find(|spec| {
            gla.skeleton[bone.bone_index].name.eq_ignore_ascii_case(spec.name)
        }).and_then(|spec| spec.end)
            .and_then(|name| bone_index(gla, name))
            .and_then(|index| bind_bone_point(gla, index))
            .unwrap_or(head);
        let rest_com = (head + end) * 0.5;
        let offset = (rest_com - anchor) * to_meters;
        let principal_inv = body.mass_properties().local_mprops.inv_principal_inertia;
        let inv_to_moment = |inv: f32| if inv > 1.0e-8 { 1.0 / inv } else { 0.0 };
        let local_inertia = (inv_to_moment(principal_inv.x)
            + inv_to_moment(principal_inv.y) + inv_to_moment(principal_inv.z)) / 3.0;
        moment += mass * offset.length_squared() + local_inertia;
    }
    moment.max(1.0e-6)
}

fn joint_both_sleeping(bodies: &RigidBodySet, joint: &SoftJoint) -> bool {
    match (bodies.get(joint.parent), bodies.get(joint.child)) {
        (Some(p), Some(c)) => p.is_sleeping() && c.is_sleeping(),
        _ => true,
    }
}

/// Split the joint rotation into swing (X,Z) and twist (Y), as in solver.py.
fn soft_joint_angles(bodies: &RigidBodySet, joint: &SoftJoint) -> Option<(Rotation, Rotation, [f32; 3])> {
    let parent = bodies.get(joint.parent)?;
    let child = bodies.get(joint.child)?;
    let qp = *parent.rotation() * joint.frame_p;
    let qc = *child.rotation() * joint.frame_c;
    let mut rel = qp.inverse() * qc;
    if rel.w < 0.0 { rel = Rotation::from_xyzw(-rel.x, -rel.y, -rel.z, -rel.w); }
    let tl = rel.w.hypot(rel.y);
    let (tw, twist) = if tl < 1.0e-6 {
        (Rotation::IDENTITY, 0.0)
    } else {
        (Rotation::from_xyzw(0.0, rel.y / tl, 0.0, rel.w / tl),
         2.0 * rel.y.atan2(rel.w))
    };
    let swing = rotation_vector(rel * tw.inverse());
    Some((qp, qc, [swing.x, swing.z, twist]))
}

fn rotation_vector(mut q: Rotation) -> Vector {
    if q.w < 0.0 { q = Rotation::from_xyzw(-q.x, -q.y, -q.z, -q.w); }
    let v = Vector::new(q.x, q.y, q.z);
    let magnitude = v.length();
    if magnitude < 1.0e-8 { return v * 2.0; }
    v * (2.0 * magnitude.atan2(q.w) / magnitude)
}

fn joint_inverse_inertia(bodies: &RigidBodySet, joint: &SoftJoint, axis: Vector) -> f32 {
    [joint.parent, joint.child].iter().filter_map(|&handle| bodies.get(handle))
        .filter(|body| body.is_dynamic())
        .map(|body| axis.dot(body.mass_properties().effective_world_inv_inertia * axis))
        .sum()
}

/// Equal and opposite torques: no artificial world-space force or root pin.
fn apply_joint_torque_impulse(bodies: &mut RigidBodySet, joint: &SoftJoint, impulse: Vector) {
    if impulse.length_squared() < 1.0e-16 || !impulse.is_finite() { return; }
    if let Some(child) = bodies.get_mut(joint.child) { child.apply_torque_impulse(impulse, true); }
    if let Some(parent) = bodies.get_mut(joint.parent) { parent.apply_torque_impulse(-impulse, true); }
}

/// Source _ang_correction: C / (w + compliance / dt²), with compliance
/// 1/(spring_strength * subtree_inertia). A pre-step angular impulse divided
/// by dt produces the equivalent first-order angular displacement in Rapier.
fn apply_soft_joint_spring(
    bodies: &mut RigidBodySet, joint: &SoftJoint,
    qp: Rotation, qc: Rotation, target: [f32; 3], stiffness: f32, h: f32,
) {
    if stiffness <= 0.0 || h <= 0.0 { return; }
    let swing = Rotation::from_scaled_axis(Vector::new(target[0], 0.0, target[1]));
    let twist = Rotation::from_axis_angle(Vector::Y, target[2]);
    let desired = qp * swing * twist;
    let error = rotation_vector(desired * qc.inverse());
    let length = error.length();
    if length <= 1.0e-9 { return; }
    let axis = error / length;
    let inv = joint_inverse_inertia(bodies, joint, axis);
    let denom = inv + 1.0 / (stiffness * h * h);
    if denom <= 1.0e-12 { return; }
    apply_joint_torque_impulse(bodies, joint, axis * (length / (h * denom)));
}

fn bone_index(gla: &GlaAnimation, name: &str) -> Option<usize> {
    gla.skeleton
        .iter()
        .position(|bone| bone.name.eq_ignore_ascii_case(name))
}

fn nearest_driven_parent(
    gla: &GlaAnimation,
    bone_index: usize,
    handles: &HashMap<usize, RigidBodyHandle>,
) -> Option<usize> {
    let mut parent = jka_assets::skeleton::mesh_pose_parent(&gla.skeleton, bone_index);
    while let Some(index) = parent {
        if handles.contains_key(&index) {
            return Some(index);
        }
        parent = jka_assets::skeleton::mesh_pose_parent(&gla.skeleton, index);
    }
    None
}

/// Resolve which physical ragdoll segment owns a Ghoul2 skinning bone.
///
/// First honor the exact Broadsword helper-effectors that should follow a major
/// physical segment, then fall back to the nearest driven skeleton ancestor.
/// This keeps stock humanoid face/twist weighting coherent without giving every
/// helper its own Rapier rigid body.
fn ragdoll_driver_for_bone(
    gla: &GlaAnimation,
    skin_bone_index: usize,
    driven_deltas: &HashMap<usize, Matrix3x4>,
) -> Option<usize> {
    fn explicit_driver_name(name: &str) -> Option<&'static str> {
        match name.to_ascii_lowercase().as_str() {
            // These are explicit Broadsword rag effectors in q_shared.h but are
            // helper/twist/skin bones rather than useful separate rigid bodies.
            "model_root" | "motion" => Some("pelvis"),
            "rradiusx" => Some("rradius"),
            "lradiusx" => Some("lradius"),
            "rfemurx" => Some("rfemurYZ"),
            "lfemurx" => Some("lfemurYZ"),
            "ceyebrow" => Some("cranium"),
            _ => None,
        }
    }

    let mut current = Some(skin_bone_index);
    while let Some(index) = current {
        if driven_deltas.contains_key(&index) {
            return Some(index);
        }
        let bone = gla.skeleton.get(index)?;
        if let Some(driver_name) = explicit_driver_name(&bone.name) {
            if let Some(driver) = bone_index(gla, driver_name) {
                if driven_deltas.contains_key(&driver) {
                    return Some(driver);
                }
            }
        }
        current = jka_assets::skeleton::mesh_pose_parent(&gla.skeleton, index);
    }
    None
}

/// Drop only scale/shear from an affine Ghoul2 bone transform while preserving
/// its model-space translation. Rapier can represent this rigid part exactly.
pub(crate) fn rigidify_affine(matrix: &Matrix3x4) -> Matrix3x4 {
    pose_to_matrix(&matrix_to_pose(matrix))
}

/// Convert Ghoul2's evaluated skinning matrix into the actual animated bone
/// transform. This matches `model_bolt_matrix` in jka-assets.
fn posed_bone_matrix(
    gla: &GlaAnimation,
    pose: &[Matrix3x4],
    bone_index: usize,
) -> Option<Matrix3x4> {
    let skinning = pose.get(bone_index)?;
    let bone = gla.skeleton.get(bone_index)?;
    Some(multiply_3x4(skinning, &bone.base_pose))
}

fn local_bone_endpoint(parent: &Matrix3x4, child: &Matrix3x4) -> Option<[f32; 3]> {
    let inverse = inverse_affine(parent)?;
    let local = multiply_3x4(&inverse, child);
    Some([local[0][3], local[1][3], local[2][3]])
}

fn lerp_matrix3x4(from: &Matrix3x4, to: &Matrix3x4, t: f32) -> Matrix3x4 {
    let mut out = [[0.0; 4]; 3];
    for row in 0..3 {
        for column in 0..4 {
            out[row][column] = from[row][column] + (to[row][column] - from[row][column]) * t;
        }
    }
    out
}

fn scale_bone_translation(matrix: &Matrix3x4, scale: f32) -> Matrix3x4 {
    let mut out = *matrix;
    out[0][3] *= scale;
    out[1][3] *= scale;
    out[2][3] *= scale;
    out
}

fn entity_matrix(axis: [[f32; 3]; 3], origin: [f32; 3]) -> Matrix3x4 {
    [
        [axis[0][0], axis[1][0], axis[2][0], origin[0]],
        [axis[0][1], axis[1][1], axis[2][1], origin[1]],
        [axis[0][2], axis[1][2], axis[2][2], origin[2]],
    ]
}

fn transform_point(matrix: &Matrix3x4, point: [f32; 3]) -> [f32; 3] {
    [
        matrix[0][0] * point[0] + matrix[0][1] * point[1] + matrix[0][2] * point[2] + matrix[0][3],
        matrix[1][0] * point[0] + matrix[1][1] * point[1] + matrix[1][2] * point[2] + matrix[1][3],
        matrix[2][0] * point[0] + matrix[2][1] * point[1] + matrix[2][2] * point[2] + matrix[2][3],
    ]
}

fn inverse_affine(matrix: &Matrix3x4) -> Option<Matrix3x4> {
    let m = glam::Mat4::from_cols(
        glam::Vec4::new(matrix[0][0], matrix[1][0], matrix[2][0], 0.0),
        glam::Vec4::new(matrix[0][1], matrix[1][1], matrix[2][1], 0.0),
        glam::Vec4::new(matrix[0][2], matrix[1][2], matrix[2][2], 0.0),
        glam::Vec4::new(matrix[0][3], matrix[1][3], matrix[2][3], 1.0),
    );
    if m.determinant().abs() <= 1.0e-8 {
        return None;
    }
    let inv = m.inverse();
    Some([
        [inv.x_axis.x, inv.y_axis.x, inv.z_axis.x, inv.w_axis.x],
        [inv.x_axis.y, inv.y_axis.y, inv.z_axis.y, inv.w_axis.y],
        [inv.x_axis.z, inv.y_axis.z, inv.z_axis.z, inv.w_axis.z],
    ])
}

fn matrix_to_pose(matrix: &Matrix3x4) -> Pose {
    // Ghoul2 lerps may leave the 3x3 very slightly non-orthonormal. Rebuild an
    // orthonormal frame before asking Rapier/glam for a quaternion.
    let mut x = Vector::new(matrix[0][0], matrix[1][0], matrix[2][0]).normalize_or_zero();
    let mut y = Vector::new(matrix[0][1], matrix[1][1], matrix[2][1]).normalize_or_zero();
    if x.length_squared() < 1.0e-8 {
        x = Vector::X;
    }
    y = (y - x * y.dot(x)).normalize_or_zero();
    if y.length_squared() < 1.0e-8 {
        let helper = if x.z.abs() < 0.9 { Vector::Z } else { Vector::Y };
        y = helper.cross(x).normalize_or_zero();
    }
    let z = x.cross(y).normalize_or_zero();
    let y = z.cross(x).normalize_or_zero();
    let rotation = Rotation::from_mat3(&Matrix::from_cols(x, y, z)).normalize();
    Pose::from_parts(
        Vector::new(matrix[0][3], matrix[1][3], matrix[2][3]) * JKA_TO_RAPIER,
        rotation,
    )
}

fn interpolate_pose(previous: &Pose, current: &Pose, alpha: f32) -> Pose {
    previous.lerp(current, alpha)
}

fn inverse_pose_point(pose: &Pose, point: Vector) -> Vector {
    pose.rotation.inverse() * (point - pose.translation)
}

fn pose_to_matrix(pose: &Pose) -> Matrix3x4 {
    let x = pose.rotation * Vector::X;
    let y = pose.rotation * Vector::Y;
    let z = pose.rotation * Vector::Z;
    let t = pose.translation * RAPIER_TO_JKA;
    [
        [x.x, y.x, z.x, t.x],
        [x.y, y.y, z.y, t.y],
        [x.z, y.z, z.z, t.z],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contact_world() -> RagdollWorld {
        let mut world = RagdollWorld::new();
        world.set_config(RagdollConfig {
            physics_enabled: true, enabled: true, lifetime_seconds: 0.1,
            ..RagdollConfig::default()
        });
        world.set_map_mesh(&PhysicsMapMesh::from_jka_mesh(
            vec![[-512.0, -512.0, 0.0], [512.0, -512.0, 0.0],
                 [512.0, 512.0, 0.0], [-512.0, 512.0, 0.0]],
            vec![[0, 1, 2], [0, 2, 3]],
        ).unwrap()).unwrap();
        world
    }

    fn contact_player(x: f32) -> PhysicsPlayer {
        PhysicsPlayer {
            entity: 0, origin: [x, 0.0, 24.0],
            mins: [-15.0, -15.0, -24.0], maxs: [15.0, 15.0, 32.0], teleport_bit: 0,
        }
    }

    #[test]
    fn corpse_skinning_scale_is_preserved_during_player_contact() {
        use jka_assets::skeleton::{humanoid_reference, HumanoidRig};
        use jka_protocol::gamestate::{EntityState, ENTITY_FIELDS};
        for rig_kind in [HumanoidRig::Jka53, HumanoidRig::Jk2_72] {
            for scale in [0.65, 1.0, 1.5] {
                let mut world = contact_world();
                let rig = humanoid_reference(rig_kind);
                let identity = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]];
                let frozen = vec![identity; rig.skeleton.len()];
                let mut state = EntityState { number: 1, fields: [0; ENTITY_FIELDS.len()] };
                state.fields[ENTITY_FIELDS.iter().position(|(name, _)| *name == "clientNum").unwrap()] = 1;
                let entity = PresentedEntity {
                    number: 1, entity_type: crate::cgame::ET_BODY,
                    origin: [0., 0., 24.], angles: [0.; 3], state,
                };
                let axis = [[0., 1., 0.], [-1., 0., 0.], [0., 0., 1.]];
                let mut pose = frozen.clone();
                world.apply_to_pose(&entity, "size-test", &rig, &mut pose, axis,
                    entity.origin, scale, RagdollMode::Corpse, None, 0).unwrap();
                let mut worst_scale_error = 0.0_f32;
                world.begin_frame(0);
                for frame in 0..300 {
                    let now = frame * 17;
                    let player = contact_player(-100.0 + frame as f32 * 2.0);
                    world.set_players(&[player], now);
                    world.begin_frame(now);
                    pose.clone_from(&frozen);
                    world.apply_to_pose(&entity, "size-test", &rig, &mut pose, axis,
                        entity.origin, scale, RagdollMode::Corpse, None, now).unwrap();
                    for index in 0..rig.skeleton.len() {
                        for column in 0..3 {
                            let length = Vector::new(pose[index][0][column], pose[index][1][column], pose[index][2][column]).length();
                            worst_scale_error = worst_scale_error.max((length - 1.0).abs());
                        }
                    }
                }
                assert!(worst_scale_error < 0.001,
                    "{rig_kind:?} scale={scale}: bone skinning scale changed by {worst_scale_error}");
            }
        }
    }

    #[test]
    fn walking_player_pushes_sleeping_articulated_corpse_after_lifetime() {
        use jka_assets::skeleton::{humanoid_reference, HumanoidRig};
        for frame_ms in [8, 16, 33] {
            let mut world = contact_world();
            let rig = humanoid_reference(HumanoidRig::Jka53);
            let identity = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]];
            let corpse = world.spawn_instance(
                "test", &rig, &vec![identity; rig.skeleton.len()],
                [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]], [0., 0., 24.], 1.0,
                CorpseGeneration { client_num: 1, body_queue_time: 0 },
                RagdollKind::Corpse, Vector::ZERO, 0,
            ).unwrap();
            let pelvis = corpse.driven[0].body;
            world.instances.insert(1, corpse);
            world.begin_frame(0);
            for time in 1..=300 { world.begin_frame(time * 17); }
            // Force a fully sleeping articulation, as with a settled corpse.
            for bone in &world.instances[&1].driven { world.bodies[bone.body].sleep(); }
            let start = world.bodies[pelvis].translation() * RAPIER_TO_JKA;
            let mut now = 5100;
            let duration = 1500;
            for elapsed in (0..=duration).step_by(frame_ms as usize) {
                now = 5100 + elapsed;
                let mut player = contact_player(start.x - 48.0 + elapsed as f32 * 0.08);
                player.origin[1] = start.y;
                world.set_players(&[player], now);
                world.begin_frame(now);
            }
            let end = world.bodies[pelvis].translation() * RAPIER_TO_JKA;
            assert!(end.x > start.x + 10.0, "{frame_ms}ms frames: {start:?} -> {end:?} at {now}");
            assert!(world.bodies[pelvis].translation().is_finite());
        }
    }

    #[test]
    fn player_hulls_accumulate_motion_and_do_not_sweep_teleports() {
        let mut world = contact_world();
        world.set_players(&[contact_player(0.0)], 0);
        world.begin_frame(0);
        let handle = world.player_colliders[&0].body;
        world.set_players(&[contact_player(1.0)], 4);
        world.begin_frame(4);
        world.set_players(&[contact_player(2.0)], 8);
        world.begin_frame(8);
        assert_eq!(world.bodies[handle].translation().x, 0.0);
        world.set_players(&[contact_player(4.0)], 17);
        world.begin_frame(17);
        assert!((world.bodies[handle].translation().x * RAPIER_TO_JKA - 4.0).abs() < 0.01);
        let mut teleported = contact_player(90.0);
        teleported.teleport_bit = 8;
        world.set_players(&[teleported], 34);
        world.begin_frame(34);
        assert!(world.bodies[handle].linvel().length() < 0.01, "teleports impart no sweep velocity");
        world.set_players(&[], 35);
        assert!(world.player_colliders.is_empty());
        assert!(world.bodies.is_empty());
    }

    #[test]
    fn player_hulls_are_cleared_on_disable_seek_and_map_change() {
        let mut world = contact_world();
        world.set_players(&[contact_player(0.0)], 0);
        world.set_config(RagdollConfig::default());
        assert!(world.player_colliders.is_empty());
        assert!(world.bodies.is_empty());
        world = contact_world();
        world.set_players(&[contact_player(0.0)], 0);
        world.reset_dynamic_for_seek();
        assert!(world.player_colliders.is_empty());
        assert!(world.bodies.is_empty());
        world.set_players(&[contact_player(0.0)], 0);
        world.set_map_mesh(&PhysicsMapMesh::default()).unwrap();
        assert!(world.player_colliders.is_empty());
        assert!(world.bodies.is_empty());
    }

    #[test]
    fn debris_uses_existing_rapier_world_and_respects_budgets_and_expiry() {
        let mut world = RagdollWorld::new();
        let spec = ChunkSpec {
            qpath: "models/chunks/rock/rock1_1.md3".into(),
            origin: [0.0, 0.0, 48.0], velocity: [24.0, 0.0, 20.0],
            angles: [0.0; 3], angle_velocity: [180.0, 30.0, 0.0],
            life_ms: 1500, bounce_factor: 0.3, scale: 1.0,
        };
        assert!(!world.spawn_debris(&[spec.clone()], 1000));
        world.set_config(RagdollConfig {
            physics_enabled: true, debris_enabled: true,
            max_debris: 2, debris_lifetime_seconds: 2.0,
            ..RagdollConfig::default()
        });
        let mesh = PhysicsMapMesh::from_jka_mesh(
            vec![[-128.0, -128.0, 0.0], [128.0, -128.0, 0.0],
                 [128.0, 128.0, 0.0], [-128.0, 128.0, 0.0]],
            vec![[0, 1, 2], [0, 2, 3]],
        ).unwrap();
        world.set_map_mesh(&mesh).unwrap();
        assert!(world.spawn_debris(&[spec.clone(), spec.clone(), spec], 1000));
        assert_eq!(world.debris.len(), 2, "oldest debris is evicted");
        let models = world.debris_models(1000);
        assert_eq!(models.len(), 2);
        // At spawn, the optional solver must preserve the same model basis
        // that the legacy JKA LE_FRAGMENT path would have drawn.
        let basis = crate::cgame::weapon_fx::rapier_debris_axis([0.0; 3]);
        for row in 0..3 {
            for col in 0..3 {
                assert!((models[0].axis[row][col] - basis[row][col]).abs() < 1.0e-4);
            }
        }
        world.begin_frame(1000);
        world.begin_frame(1050);
        assert_eq!(world.debris_models(1050).len(), 2);
        world.begin_frame(3001);
        assert!(world.debris.is_empty(), "expired bodies leave the shared world");
        assert_eq!(world.bodies.len(), 0);
    }

    #[test]
    fn glass_delays_activation_keeps_original_quad_and_releases_on_time() {
        let mut world = RagdollWorld::new();
        let spec = ShardSpec {
            positions: [[0.0, 0.0, 48.0], [12.0, 0.0, 48.0],
                        [12.0, 0.0, 60.0], [0.0, 0.0, 60.0]],
            uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            velocity: [4.0, 0.0, 0.0], accel_z: -650.0,
            rotation: [15.0, -30.0], bounce: 0.2, delay_ms: 100,
        };
        assert_eq!(world.spawn_glass(&[spec.clone()], 1000), 0);
        world.set_config(RagdollConfig {
            physics_enabled: true, debris_enabled: true,
            max_debris: 2, debris_lifetime_seconds: 6.0,
            ..RagdollConfig::default()
        });
        let mesh = PhysicsMapMesh::from_jka_mesh(
            vec![[-128.0, -128.0, 0.0], [128.0, -128.0, 0.0],
                 [128.0, 128.0, 0.0], [-128.0, 128.0, 0.0]],
            vec![[0, 1, 2], [0, 2, 3]],
        ).unwrap();
        world.set_map_mesh(&mesh).unwrap();
        assert_eq!(world.spawn_glass(&[spec.clone(), spec.clone(), spec], 1000), 2);
        assert_eq!(world.glass.len(), 2);
        assert!(world.glass.iter().all(|g| g.body.is_none()));
        assert_eq!(world.glass_draws(1000).len(), 2);
        world.begin_frame(1000);
        world.begin_frame(1050);
        assert!(world.glass.iter().all(|g| g.body.is_none()));
        world.begin_frame(1100);
        assert!(world.glass.iter().all(|g| g.body.is_some()));
        world.reset_dynamic_for_seek();
        assert!(world.glass.is_empty());
    }

    #[test]
    fn native_jk2_ragdoll_digits_and_toes_follow_their_physical_limbs() {
        use jka_assets::skeleton::{humanoid_reference, HumanoidRig};
        let rig = humanoid_reference(HumanoidRig::Jk2_72);
        let wrist = bone_index(&rig, "rhand").unwrap();
        let ankle = bone_index(&rig, "ltalus").unwrap();
        let torso = bone_index(&rig, "thoracic").unwrap();
        let identity = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]];
        let deltas = HashMap::from([(wrist, identity), (ankle, identity), (torso, identity)]);
        for name in ["r_d3_j3", "r_d5_j1", "mc7", "rhang_tag_bone"] {
            assert_eq!(ragdoll_driver_for_bone(&rig, bone_index(&rig, name).unwrap(), &deltas), Some(wrist));
        }
        assert_eq!(ragdoll_driver_for_bone(&rig, bone_index(&rig, "ltarsal").unwrap(), &deltas), Some(ankle));
        let upper_arm = bone_index(&rig, "rhumerus").unwrap();
        let forearm = bone_index(&rig, "rradius").unwrap();
        let mut bodies = RigidBodySet::new();
        let handles = HashMap::from([
            (upper_arm, bodies.insert(RigidBodyBuilder::dynamic().build())),
            (torso, bodies.insert(RigidBodyBuilder::dynamic().build())),
        ]);
        assert_eq!(nearest_driven_parent(&rig, forearm, &handles), Some(upper_arm));
        let jka = humanoid_reference(HumanoidRig::Jka53);
        for (i, bone) in jka.skeleton.iter().enumerate() {
            assert_eq!(jka_assets::skeleton::mesh_pose_parent(&jka.skeleton, i), usize::try_from(bone.parent).ok());
        }
    }

    #[test]
    fn source_preset_and_zone_match_blender() {
        let (elbow, down, parent) = soft_joint_preset("lradius").unwrap();
        assert!(!down && parent);
        assert!((elbow[0][0] - 0.0).abs() < 0.001);
        assert!((elbow[0][1] - 145_f32.to_radians()).abs() < 0.001);
        assert!((elbow[0][2] - 65_f32.to_radians()).abs() < 0.001);
        let (mirrored, _, _) = soft_joint_preset("rradius").unwrap();
        assert_eq!(mirrored[1][0], -elbow[1][1]);
        assert_eq!(mirrored[1][1], -elbow[1][0]);
        assert!(soft_joint_preset("rtarsal").is_none());
    }

    #[test]
    fn anatomical_soft_frames_exist_on_both_ghoul2_skeletons() {
        use jka_assets::skeleton::{humanoid_reference, HumanoidRig};
        for rig in [HumanoidRig::Jka53, HumanoidRig::Jk2_72] {
            let gla = humanoid_reference(rig);
            for name in ["rradius", "lradius", "rtibia", "ltibia", "rhumerus", "lfemurYZ"] {
                let child = bone_index(&gla, name).unwrap();
                let mut bodies = RigidBodySet::new();
                let handles = BODY_SPECS.iter().filter_map(|spec| {
                    bone_index(&gla, spec.name)
                        .map(|i| (i, bodies.insert(RigidBodyBuilder::dynamic().build())))
                }).collect::<HashMap<_, _>>();
                let parent = nearest_driven_parent(&gla, child, &handles).unwrap();
                let (fp, fc, _) = anatomical_soft_joint(&gla, parent, child, name).unwrap();
                assert!(fp.is_finite() && fc.is_finite(), "{rig:?} {name}");
            }
        }
    }

    #[test]
    fn soft_joint_springs_do_not_push_or_pin_a_free_falling_corpse() {
        let mut bodies = RigidBodySet::new();
        let parent = bodies.insert(RigidBodyBuilder::dynamic().translation(Vector::new(0.0, 0.0, 5.0)).build());
        let child = bodies.insert(RigidBodyBuilder::dynamic().translation(Vector::new(0.0, 0.0, 4.0)).build());
        let mut colliders = ColliderSet::new();
        for handle in [parent, child] {
            colliders.insert_with_parent(ColliderBuilder::ball(0.2).mass(2.0), handle, &mut bodies);
        }
        let joint = SoftJoint {
            parent, child, frame_p: Rotation::IDENTITY, frame_c: Rotation::IDENTITY,
            ranges: [[-1.0, 1.0, 0.5], [-1.0, 1.0, 0.0], [-1.0, 1.0, 0.0]],
            effective_inertia: 1.0,
        };
        apply_soft_joint_spring(&mut bodies, &joint, Rotation::IDENTITY, Rotation::IDENTITY,
            [0.5, 0.0, 0.0], 10.0, 1.0 / 60.0);
        // Pure angular impulses cannot create upward force or net linear
        // momentum in midair; only gravity and contacts move the COM.
        assert_eq!(bodies[parent].linvel(), Vector::ZERO);
        assert_eq!(bodies[child].linvel(), Vector::ZERO);
    }

    #[test]
    fn both_joint_modes_fall_under_gravity_after_death() {
        use jka_assets::skeleton::{humanoid_reference, HumanoidRig};
        let rig = humanoid_reference(HumanoidRig::Jka53);
        let identity = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]];
        for joint_model in [0, 1] {
            let mut world = contact_world();
            world.set_config(RagdollConfig {
                physics_enabled: true, enabled: true,
                joint_model, lifetime_seconds: 20.0,
                ..RagdollConfig::default()
            });
            let corpse = world.spawn_instance(
                "test", &rig, &vec![identity; rig.skeleton.len()],
                [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]], [0., 0., 96.], 1.0,
                CorpseGeneration { client_num: 1, body_queue_time: 0 },
                RagdollKind::Corpse, Vector::ZERO, 0,
            ).unwrap();
            let pelvis = corpse.driven.iter()
                .find(|driven| rig.skeleton[driven.bone_index].name.eq_ignore_ascii_case("pelvis"))
                .unwrap().body;
            let start_z = world.bodies[pelvis].translation().z;
            world.instances.insert(1, corpse);
            world.begin_frame(0);
            for tick in 1..=25 { world.begin_frame(tick * 17); }
            let end_z = world.bodies[pelvis].translation().z;
            assert!(end_z.is_finite() && end_z < start_z - 0.2,
                "joint_model={joint_model}: corpse failed to fall under gravity: {start_z} -> {end_z}");
        }
    }

    #[test]
    fn mode_switch_rebuilds_articulations_and_removes_old_joint_motors() {
        use jka_assets::skeleton::{humanoid_reference, HumanoidRig};
        let rig = humanoid_reference(HumanoidRig::Jka53);
        let identity = [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]];
        let mut world = contact_world();
        world.set_config(RagdollConfig {
            physics_enabled: true, enabled: true, joint_model: 1,
            ..RagdollConfig::default()
        });
        let active = world.spawn_instance(
            "test", &rig, &vec![identity; rig.skeleton.len()],
            [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]], [0., 0., 32.], 1.0,
            CorpseGeneration { client_num: 1, body_queue_time: 0 },
            RagdollKind::Corpse, Vector::ZERO, 0,
        ).unwrap();
        assert!(!active.soft_joints.is_empty());
        world.instances.insert(1, active);
        assert!(!world.impulse_joints.is_empty());
        world.set_config(RagdollConfig {
            physics_enabled: true, enabled: true, joint_model: 0,
            ..RagdollConfig::default()
        });
        assert!(world.instances.is_empty());
        assert!(world.impulse_joints.is_empty());
        let classic = world.spawn_instance(
            "test", &rig, &vec![identity; rig.skeleton.len()],
            [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]], [0., 0., 32.], 1.0,
            CorpseGeneration { client_num: 1, body_queue_time: 0 },
            RagdollKind::Corpse, Vector::ZERO, 0,
        ).unwrap();
        assert!(classic.soft_joints.is_empty());
        assert!(!world.impulse_joints.is_empty());
    }

    #[test]
    fn affine_inverse_round_trip() {
        let a = [
            [0.0, -1.0, 0.0, 12.0],
            [1.0, 0.0, 0.0, -5.0],
            [0.0, 0.0, 1.0, 33.0],
        ];
        let inv = inverse_affine(&a).unwrap();
        let identity = multiply_3x4(&inv, &a);
        for row in 0..3 {
            for col in 0..4 {
                let expected = if row == col { 1.0 } else { 0.0 };
                assert!((identity[row][col] - expected).abs() < 1.0e-4);
            }
        }
    }

    #[test]
    fn pose_round_trip_preserves_rigid_transform() {
        let m = [
            [0.0, -1.0, 0.0, 32.0],
            [1.0, 0.0, 0.0, 64.0],
            [0.0, 0.0, 1.0, 96.0],
        ];
        let out = pose_to_matrix(&matrix_to_pose(&m));
        for row in 0..3 {
            for col in 0..4 {
                assert!((out[row][col] - m[row][col]).abs() < 1.0e-4);
            }
        }
    }
    #[test]
    fn ragdoll_delta_bridge_preserves_exact_spawn_skinning_pose() {
        // Deliberately include a tiny non-rigid interpolation term. Rapier must
        // not normalize this out of the visible pose merely by taking ownership.
        let spawn_skin = [
            [0.999, -0.030, 0.002, 12.0],
            [0.031, 0.998, -0.004, -7.0],
            [0.001, 0.005, 1.001, 33.0],
        ];
        let spawn_actual = [
            [0.998, -0.040, 0.003, 20.0],
            [0.041, 0.997, -0.006, -3.0],
            [0.002, 0.007, 1.002, 41.0],
        ];
        let rigid = rigidify_affine(&spawn_actual);
        let delta = multiply_3x4(&rigid, &inverse_affine(&rigid).unwrap());
        let out = multiply_3x4(&delta, &spawn_skin);
        for row in 0..3 {
            for col in 0..4 {
                assert!((out[row][col] - spawn_skin[row][col]).abs() < 1.0e-4);
            }
        }
    }

    #[test]
    fn model_scale_round_trip_does_not_rescale_bone_space() {
        let bone = [
            [0.0, -1.0, 0.0, 12.0],
            [1.0, 0.0, 0.0, -8.0],
            [0.0, 0.0, 1.0, 36.0],
        ];
        let axis = [
            [0.0, 1.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
        ];
        let entity = entity_matrix(axis, [100.0, 200.0, 24.0]);
        let model_scale = 1.5;
        let scaled = scale_bone_translation(&bone, model_scale);
        let world = multiply_3x4(&entity, &scaled);
        let rapier_world = pose_to_matrix(&matrix_to_pose(&world));
        let model_scaled = multiply_3x4(&inverse_affine(&entity).unwrap(), &rapier_world);
        let model = scale_bone_translation(&model_scaled, 1.0 / model_scale);
        let expected = rigidify_affine(&bone);
        for row in 0..3 {
            for col in 0..4 {
                assert!((model[row][col] - expected[row][col]).abs() < 1.0e-4);
            }
        }
    }

}

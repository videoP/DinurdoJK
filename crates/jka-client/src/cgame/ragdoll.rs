//! Rapier-backed client-side ragdolls for JKA Ghoul2 corpses.
//!
//! OpenJK's old Broadsword path enters ragdoll presentation for dead entities /
//! `EF_RAG` immediately before Ghoul2 submission. This module keeps that
//! ownership boundary, but replaces Broadsword's renderer-side solver with
//! Rapier.  Network snapshots and OpenJK pmove remain authoritative.

use crate::cgame::PresentedEntity;
use jka_assets::ghoul2::{multiply_3x4, GlaAnimation, Matrix3x4};
use rapier3d::{
    math::{Matrix, Pose, Rotation, Vector},
    prelude::*,
};
use std::collections::{HashMap, HashSet};

/// JKA uses large Quake-style map units. Keeping Rapier near meter-ish scales
/// improves solver tolerances without changing anything visible to the game.
const JKA_TO_RAPIER: f32 = 1.0 / 32.0;
const RAPIER_TO_JKA: f32 = 1.0 / JKA_TO_RAPIER;
const FORCE_GRIP_RECOVERY_MS: i32 = 180;
const JKA_GRAVITY: f32 = 800.0;

#[derive(Debug, Clone, Copy)]
pub struct RagdollConfig {
    pub enabled: bool,
    pub hz: u32,
    pub max_substeps: u32,
    pub ccd: bool,
    pub sleeping: bool,
    pub max_ragdolls: u32,
    pub lifetime_seconds: f32,
    pub self_collision: bool,
    pub debug: bool,
    pub stats: bool,
}

impl Default for RagdollConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            hz: 60,
            max_substeps: 4,
            ccd: true,
            sleeping: true,
            max_ragdolls: 8,
            lifetime_seconds: 20.0,
            self_collision: false,
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
enum RagdollKind {
    Corpse,
    ForceGrip,
}

#[derive(Debug, Clone, Copy)]
struct GripAnchor {
    /// Position-based kinematic body driven by the authoritative victim entity.
    body: RigidBodyHandle,
    /// Frozen cervical/neck position in unscaled Ghoul2 model space.
    neck_model_position: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CorpseGeneration {
    client_num: i32,
    /// Body-queue slots are reused, so ET_BODY needs one stable copy stamp.
    /// Live dead ET_PLAYER/ET_NPC entities deliberately use zero here: their
    /// server trajectory keeps changing while the same death is in progress.
    body_queue_time: i32,
}

#[derive(Debug, Clone)]
struct ForceGripRecovery {
    model_key: String,
    started_at: i32,
    from_pose: Vec<Matrix3x4>,
}

#[derive(Debug)]
struct RagdollInstance {
    model_key: String,
    model_scale: f32,
    generation: CorpseGeneration,
    kind: RagdollKind,
    grip_anchor: Option<GripAnchor>,
    spawned_at: i32,
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
    /// ragdoll. Force Grip release captures this exact visual pose and blends
    /// it back to live animation instead of snapping on the first non-grip
    /// frame.
    last_applied_pose: Vec<Matrix3x4>,
    driven: Vec<DrivenBone>,
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
    force_grip_recoveries: HashMap<u16, ForceGripRecovery>,
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
            force_grip_recoveries: HashMap::new(),
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

    pub fn debug_enabled(&self) -> bool {
        self.config.debug
    }

    pub fn set_config(&mut self, config: RagdollConfig) {
        let was_enabled = self.config.enabled;
        let debug_was = self.config.debug;
        let rebuild_dynamic = self.config.enabled
            && config.enabled
            && (self.config.ccd != config.ccd
                || self.config.sleeping != config.sleeping
                || self.config.self_collision != config.self_collision);
        self.config = RagdollConfig {
            hz: config.hz.clamp(30, 240),
            max_substeps: config.max_substeps.clamp(1, 16),
            max_ragdolls: config.max_ragdolls.min(64),
            lifetime_seconds: config.lifetime_seconds.clamp(0.0, 300.0),
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
        }
        if !was_enabled && self.config.enabled {
            self.last_time = None;
            self.accumulator = 0.0;
        }
        // These flags are baked into already-created rigid bodies/colliders.
        // Re-seed visible corpses from their current authoritative animation
        // pose instead of leaving a mixed old/new physics world.
        if rebuild_dynamic {
            self.clear_instances();
        }
        self.trim_to_budget();
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
        println!(
            "RAPIER MAP COLLISION: {} vertices / {} triangles",
            mesh.vertex_count,
            mesh.triangle_count
        );
        Ok(())
    }

    pub fn begin_frame(&mut self, current_time: i32) {
        self.force_grip_recoveries.retain(|_, recovery| {
            current_time.saturating_sub(recovery.started_at) < FORCE_GRIP_RECOVERY_MS
        });
        let Some(previous) = self.last_time.replace(current_time) else {
            return;
        };
        if !self.active() {
            self.accumulator = 0.0;
            return;
        }
        if current_time < previous {
            // Demo seeking/reverse playback invalidates every integrated pose.
            // The next ET_BODY presentation will seed each corpse from the
            // authoritative snapshot/death animation pose at the new time.
            self.clear_instances();
            self.accumulator = 0.0;
            return;
        }
        if current_time == previous {
            // Paused demo/game time: keep the existing interpolation fraction
            // so the rendered corpse freezes exactly where it was.
            return;
        }

        self.sleep_expired(current_time);

        // The presentation clock can jump while loading/fast-forwarding. Never
        // try to pay back an arbitrarily large physics debt in one frame.
        let frame_dt = ((current_time - previous) as f32 * 0.001).min(0.1);
        let step_dt = 1.0 / self.config.hz.max(1) as f32;
        self.integration.dt = step_dt;
        self.accumulator += frame_dt;

        let mut steps = 0;
        while self.accumulator + 1.0e-6 >= step_dt && steps < self.config.max_substeps {
            self.capture_previous_poses();
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
                "RAPIER STATS: active={} ragdolls={} bodies={} colliders={} joints={} steps={} accumulatorMs={:.2}",
                self.active(),
                self.instances.len(),
                self.bodies.len(),
                self.colliders.len(),
                self.impulse_joints.len(),
                steps,
                self.accumulator * 1000.0,
            );
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
        force_grip: bool,
        current_time: i32,
    ) -> Result<bool, String> {
        let generation = corpse_generation(entity);
        let kind = if force_grip { RagdollKind::ForceGrip } else { RagdollKind::Corpse };
        // Any new physics takeover supersedes a pending live-animation
        // recovery for this entity (re-grip or death during recovery).
        self.force_grip_recoveries.remove(&entity.number);
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
                RagdollKind::ForceGrip => 1.0,
                RagdollKind::Corpse if grounded => 0.0,
                RagdollKind::Corpse => 2.0,
            };
            let velocity = Vector::new(
                entity.state.field_f32("pos.trDelta[0]").unwrap_or(0.0),
                entity.state.field_f32("pos.trDelta[1]").unwrap_or(0.0),
                entity.state.field_f32("pos.trDelta[2]").unwrap_or(0.0),
            ) * (JKA_TO_RAPIER * velocity_scale);
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
            println!(
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

    /// Begin a short visual recovery from the final Force-grip ragdoll pose to
    /// the authoritative live Ghoul2 animation. Gameplay state changes
    /// immediately; only presentation is blended.
    pub fn begin_force_grip_recovery(&mut self, entity: u16, current_time: i32) {
        let Some(instance) = self.instances.get(&entity) else {
            return;
        };
        if instance.kind != RagdollKind::ForceGrip || instance.last_applied_pose.is_empty() {
            return;
        }
        let recovery = ForceGripRecovery {
            model_key: instance.model_key.clone(),
            started_at: current_time,
            from_pose: instance.last_applied_pose.clone(),
        };
        self.force_grip_recoveries.insert(entity, recovery);
        self.remove_instance(entity);
        if self.config.debug {
            println!(
                "RAPIER FORCE GRIP RECOVERY: victim={} durationMs={}",
                entity, FORCE_GRIP_RECOVERY_MS
            );
        }
    }

    /// Blend the exact final rendered ragdoll pose into this frame's normal
    /// living animation. Ghoul2 itself uses component-wise 3x4 matrix lerps
    /// for animation blends; use the same convention here for a brief handoff.
    pub fn apply_force_grip_recovery(
        &mut self,
        entity: u16,
        model_key: &str,
        pose: &mut [Matrix3x4],
        current_time: i32,
    ) -> bool {
        let Some(recovery) = self.force_grip_recoveries.get(&entity).cloned() else {
            return false;
        };
        if recovery.model_key != model_key || recovery.from_pose.len() != pose.len() {
            self.force_grip_recoveries.remove(&entity);
            return false;
        }

        let elapsed = current_time.saturating_sub(recovery.started_at);
        if elapsed >= FORCE_GRIP_RECOVERY_MS {
            self.force_grip_recoveries.remove(&entity);
            return false;
        }
        let t = (elapsed.max(0) as f32 / FORCE_GRIP_RECOVERY_MS as f32).clamp(0.0, 1.0);
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
            let body = self.bodies.insert(
                RigidBodyBuilder::dynamic()
                    .pose(world_pose)
                    .linvel(velocity)
                    .linear_damping(0.8)
                    .angular_damping(4.5)
                    .ccd_enabled(self.config.ccd)
                    .can_sleep(self.config.sleeping)
                    .additional_solver_iterations(8),
            );

            if let Some(spawned) = self.bodies.get(body) {
                self.previous_body_poses
                    .insert(body, spawned.position().clone());
            }

            let dynamic_groups = if self.config.self_collision {
                InteractionGroups::new(
                    Group::GROUP_2,
                    Group::GROUP_1 | Group::GROUP_2,
                    InteractionTestMode::default(),
                )
            } else {
                InteractionGroups::new(Group::GROUP_2, Group::GROUP_1, InteractionTestMode::default())
            };
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

            // Give the spherical joint a reference orientation matching the
            // exact death pose. Limits are therefore relative to the handoff
            // pose instead of snapping every limb toward an arbitrary identity
            // orientation on the first solver step.
            let parent_frame_rotation =
                parent_body.position().rotation.inverse() * child_body.position().rotation;
            let limits = joint_limits_for_bone(
                gla.skeleton
                    .get(child.bone_index)
                    .map(|bone| bone.name.as_str())
                    .unwrap_or(""),
            );
            let joint = SphericalJointBuilder::new()
                .local_frame1(Pose::from_parts(parent_anchor, parent_frame_rotation))
                .local_frame2(Pose::from_parts(child_anchor, Rotation::IDENTITY))
                .limits(JointAxis::AngX, limits[0])
                .limits(JointAxis::AngY, limits[1])
                .limits(JointAxis::AngZ, limits[2])
                // Adjacent capsules overlap by design around the joint.
                .contacts_enabled(false);
            self.impulse_joints
                .insert(parent_handle, child.body, joint, true);
        }

        Ok(RagdollInstance {
            model_key: model_key.to_owned(),
            model_scale,
            generation,
            kind,
            grip_anchor,
            spawned_at: current_time,
            last_authoritative_origin: origin,
            last_applied_pose: spawn_pose.clone(),
            spawn_pose,
            driven,
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
    }

    fn interpolation_alpha(&self) -> f32 {
        let step_dt = 1.0 / self.config.hz.max(1) as f32;
        (self.accumulator / step_dt).clamp(0.0, 1.0)
    }

    fn sleep_expired(&mut self, current_time: i32) {
        if self.config.lifetime_seconds <= 0.0 {
            return;
        }
        let lifetime_ms = (self.config.lifetime_seconds * 1000.0) as i32;
        for instance in self.instances.values() {
            if current_time.saturating_sub(instance.spawned_at) < lifetime_ms {
                continue;
            }
            for driven in &instance.driven {
                if let Some(body) = self.bodies.get_mut(driven.body) {
                    body.sleep();
                }
            }
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
        self.force_grip_recoveries.clear();
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
        self.force_grip_recoveries.clear();
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
    let mut parent = gla.skeleton.get(bone_index)?.parent;
    while let Ok(index) = usize::try_from(parent) {
        if handles.contains_key(&index) {
            return Some(index);
        }
        parent = gla.skeleton.get(index)?.parent;
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
        current = usize::try_from(bone.parent).ok();
    }
    None
}

/// Drop only scale/shear from an affine Ghoul2 bone transform while preserving
/// its model-space translation. Rapier can represent this rigid part exactly.
fn rigidify_affine(matrix: &Matrix3x4) -> Matrix3x4 {
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

//! Experimental presentation-only cloth for Ghoul2 player surfaces.
//!
//! This intentionally lives downstream of JKA prediction/gameplay. It takes the
//! already-posed Ghoul2 garment surfaces, stitches authored surface boundaries
//! into one physical garment and uses `rapier-cloth-core` for secondary offsets.
//! The current skinned pose remains the rendered reference, so skeletal edge
//! deformation cannot fight fixed cloth rest lengths. Native contacts query the
//! posed garment. The original render surfaces/UVs/materials stay authoritative.

use rapier_cloth_core::{
    Cloth, ClothError, ClothMaterial, ClothMesh, Contact, ContactKey, ContactSource, ContactStage,
    Real, Solver, SolverSettings, StepReport, Target, Vec3,
};
use std::collections::{HashMap, HashSet, VecDeque};

/// JKA world units are inches closely enough for presentation physics. Keeping
/// the solver in metres also keeps gravity/material defaults in their intended
/// range.
const METRES_PER_JKA_UNIT: f32 = 0.0254;
const TELEPORT_DISTANCE_METRES: f32 = 1.5;
/// Last-resort safety net for malformed/custom surfaces or an unstable step.
/// Cloth is simulated in player-local space, so a vertex this far from its
/// corresponding animated pose is unquestionably runaway state.
const RUNAWAY_DISTANCE_METRES: f32 = 1.0;
/// Auto-promoted GLM garments were authored for skeletal skinning, not as
/// completely free cloth. Use the solver's native XPBD target constraints to
/// keep secondary displacement near its reference. The correction strength
/// falls with geodesic distance from sparse attachment points so waist/shoulder
/// regions remain controlled while cuffs/skirt tails are free to lag and swing.
/// Unlike the old post-step max-distance clamp, these targets participate in
/// the same solve as stretch/bend/contact constraints and therefore do not
/// teleport particles outside the solver or inject correction energy.
const FOLLOW_NEAR_STRENGTH: f32 = 0.18;
const FOLLOW_FAR_STRENGTH: f32 = 0.025;
const FOLLOW_FALLOFF_JKA: f32 = 32.0;
const MAX_DRIVE_ACCELERATION: f32 = 35.0;
const SECONDARY_GRAVITY: f32 = 1.0;
const INSTANCE_TTL_MS: i32 = 5_000;
const DEBUG_INTERVAL_MS: i32 = 500;

/// Authored Ghoul2 garments are often split at body-part/material boundaries.
/// Those pieces are not guaranteed to have byte-identical seam coordinates.
/// One JKA unit is still tiny relative to a player, and matching is restricted
/// to boundary vertices on *different* cloth surfaces with one nearest match per
/// surface pair, so this is intentionally much looser than the intra-surface UV
/// seam weld below without indiscriminately fusing nearby layers of one surface.
const CROSS_SURFACE_WELD_DISTANCE_JKA: f32 = 1.0;
const INTRA_SURFACE_WELD_SCALE: f32 = 1024.0;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ClothConfig {
    pub enabled: bool,
    pub hz: u32,
    pub max_substeps: u32,
    pub body_collision: bool,
    pub body_clearance: f32,
    pub air_resistance: f32,
    pub turn_response: f32,
    pub animation_influence: f32,
    /// World JKA units/second, sampled from shared weather wind.
    pub wind_velocity: [f32; 3],
}

impl Default for ClothConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            hz: 60,
            max_substeps: 4,
            body_collision: true,
            body_clearance: 1.0,
            air_resistance: 1.0,
            turn_response: 1.8,
            animation_influence: 0.35,
            wind_velocity: [0.0; 3],
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ClothCapsule {
    /// Ghoul2/player-local JKA units. Keeping the solver in player-local space
    /// prevents ordinary root translation/yaw from being injected as enormous
    /// stretch energy into an otherwise small garment.
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub radius: f32,
    /// Optional world/local JKA-unit axes of a unit-radius stretched capsule.
    pub basis: Option<[[f32; 3]; 3]>,
}

/// Actual render transform, including model scale. World motion is sampled at
/// fixed ticks; the physical state and secondary offsets remain player-local.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ClothMotion {
    pub axis: [[f32; 3]; 3],
    pub origin: [f32; 3],
}

impl ClothMotion {
    fn matrix(self) -> glam::Mat3 {
        glam::Mat3::from_cols_array_2d(&self.axis)
    }

    fn interpolate(self, other: Self, t: f32) -> Self {
        Self {
            axis: {
                // Linear matrix interpolation becomes singular halfway through
                // a 180-degree turn. Interpolate rotation and scale separately.
                let (a_scale, a_rotation, _) =
                    glam::Mat4::from_mat3(self.matrix()).to_scale_rotation_translation();
                let (b_scale, b_rotation, _) =
                    glam::Mat4::from_mat3(other.matrix()).to_scale_rotation_translation();
                (glam::Mat3::from_quat(a_rotation.slerp(b_rotation, t))
                    * glam::Mat3::from_diagonal(a_scale.lerp(b_scale, t)))
                .to_cols_array_2d()
            },
            origin: std::array::from_fn(|i| {
                self.origin[i] + (other.origin[i] - self.origin[i]) * t
            }),
        }
    }
}

pub(crate) struct ClothOutput {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
}

/// One authored GLM surface participating in this frame's garment. This is
/// intentionally render-surface-shaped: simulation merges the topology, then
/// scatters solved positions back into these original surfaces so shaders/UVs
/// and draw calls remain unchanged.
pub(crate) struct ClothSurfaceFrame {
    pub surface_index: usize,
    pub surface_name: String,
    pub bind_positions: Vec<[f32; 3]>,
    pub posed_positions: Vec<[f32; 3]>,
    pub posed_normals: Vec<[f32; 3]>,
    pub skin_transforms: Vec<SkinTransform>,
    pub triangles: Vec<[u32; 3]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ClothKey {
    entity_num: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SurfaceSignature {
    surface_index: usize,
    surface_name: String,
    vertex_count: usize,
    triangle_count: usize,
}

struct ClothSurfaceLayout {
    surface_index: usize,
    render_to_particle: Vec<u32>,
}

struct ClothInstance {
    model_label: String,
    lod_index: usize,
    signature: Vec<SurfaceSignature>,
    cloth: Cloth,
    solver: Solver,
    /// Each physical particle maps back to flattened render vertices from one
    /// or more GLM surfaces. Cross-surface seam welding therefore follows the
    /// average authoritative animation target for all authored copies.
    particle_members: Vec<Vec<usize>>,
    layouts: Vec<ClothSurfaceLayout>,
    pinned: Vec<u32>,
    /// Fraction of isolated target error corrected at the reference 60Hz rate.
    /// Hard-pinned particles store zero because pins are supplied by
    /// Cloth itself and external targets for them would conflict.
    follow_strengths: Vec<f32>,
    /// Fixed reference plus secondary displacement. Skinned edge strain never
    /// enters XPBD; animation is added at output and in native contact queries.
    reference: Vec<Vec3>,
    bind_reference: Vec<Vec3>,
    driver_particles: Vec<u32>,
    attachment_drivers: Vec<Vec<(usize, f32)>>,
    animation_freedom: Vec<f32>,
    clearance_weights: Vec<f32>,
    contact_limits: Vec<f32>,
    frame_targets: Vec<Vec3>,
    frame_authored_targets: Vec<Vec3>,
    wind_noise: fastnoise_lite::FastNoiseLite,
    wind_time: f32,
    frame_motion: ClothMotion,
    frame_capsules: Vec<ClothCapsule>,
    tick_targets: Vec<Vec3>,
    tick_motion: ClothMotion,
    drive_velocities: Option<Vec<Vec3>>,
    turn_velocities: Option<Vec<Vec3>>,
    /// Unique physical edges with their initial rest lengths. These are only
    /// retained for developer telemetry so we can see when the *animated*
    /// Ghoul2 reference pose itself is asking the fixed-rest cloth topology to
    /// stretch far beyond its authored starting shape.
    debug_edges: Vec<(u32, u32, f32)>,
    accumulator: f32,
    last_time_ms: i32,
    last_seen_ms: i32,
    last_debug_ms: i32,
    last_report: Option<StepReport>,
    rejected_steps: u32,
}

#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)] // Read by the offline replay which includes this source.
pub(crate) struct ClothDiagnostics {
    pub particles: usize,
    pub pinned: usize,
    pub max_secondary_offset_units: f32,
    pub mean_secondary_offset_units: f32,
    pub rejected_steps: u32,
}

pub(crate) struct ClothSystem {
    config: ClothConfig,
    instances: HashMap<ClothKey, ClothInstance>,
    reported_failures: HashSet<String>,
}

impl Default for ClothSystem {
    fn default() -> Self {
        Self {
            config: ClothConfig::default(),
            instances: HashMap::new(),
            reported_failures: HashSet::new(),
        }
    }
}

impl ClothSystem {
    pub fn set_config(&mut self, config: ClothConfig) {
        if (self.config.enabled && !config.enabled)
            || self.config.animation_influence != config.animation_influence
            || self.config.body_clearance != config.body_clearance
            || self.config.body_collision != config.body_collision
        {
            self.instances.clear();
        }
        self.config = config;
    }

    pub fn set_wind_velocity(&mut self, velocity: [f32; 3]) {
        self.config.wind_velocity = velocity;
    }

    /// Also used by the offline replay so its stability gates inspect physical
    /// offsets, not only the rendered pose after animation blending.
    #[allow(dead_code)]
    pub(crate) fn diagnostics(&self, entity_num: u16) -> Option<ClothDiagnostics> {
        let instance = self.instances.get(&ClothKey { entity_num })?;
        let mut max = 0.0_f32;
        let mut total = 0.0;
        for (position, reference) in instance.cloth.positions().iter().zip(&instance.reference) {
            let distance = (*position - *reference).length() / METRES_PER_JKA_UNIT;
            max = max.max(distance);
            total += distance;
        }
        Some(ClothDiagnostics {
            particles: instance.reference.len(),
            pinned: instance.pinned.len(),
            max_secondary_offset_units: max,
            mean_secondary_offset_units: total / instance.reference.len().max(1) as f32,
            rejected_steps: instance.rejected_steps,
        })
    }

    pub fn reset(&mut self) {
        self.instances.clear();
    }

    pub fn begin_frame(&mut self, now_ms: i32) {
        if !self.config.enabled {
            return;
        }
        // Demo seeks can move the presentation clock backwards. Throw away
        // temporal cloth state rather than trying to integrate through a seek.
        if self
            .instances
            .values()
            .any(|instance| now_ms < instance.last_time_ms)
        {
            self.instances.clear();
            return;
        }
        self.instances
            .retain(|_, instance| now_ms.saturating_sub(instance.last_seen_ms) <= INSTANCE_TTL_MS);
    }

    #[inline]
    pub fn enabled(&self) -> bool {
        self.config.enabled
    }

    /// Deliberately conservative V1 auto-promotion. Do not infer cloth from a
    /// shader/texture name: GLM hierarchy surface names describe the geometry.
    pub fn is_cloth_surface_name(name: &str) -> bool {
        let name = name.to_ascii_lowercase();
        // Ghoul2 character models commonly contain tiny *_robecap_* surfaces
        // that merely close holes where independently-authored body sections
        // meet. They contain "robe" in the name but are not garment cloth.
        // Promoting them creates lots of tiny stitched pieces and, more
        // importantly, used to create extra animation pins all over the robe.
        let cap_surface = [
            "robecap",
            "cloakcap",
            "capecap",
            "robe_cap",
            "cloak_cap",
            "cape_cap",
            "cap_robe",
            "cap_cloak",
            "cap_cape",
        ]
        .iter()
        .any(|needle| name.contains(needle));
        !cap_surface
            && ["cape", "cloak", "robe"]
                .iter()
                .any(|needle| name.contains(needle))
    }

    /// Simulate all active cloth surfaces for one player as one garment.
    ///
    /// The important invariant is that GLM surface boundaries are a rendering
    /// detail, not a physics boundary. A robe authored as hips_robe + torso_robe
    /// + shoulder/arm pieces therefore shares one XPBD particle topology.
    pub fn simulate_garment(
        &mut self,
        entity_num: u16,
        model_label: &str,
        lod_index: usize,
        surfaces: &[ClothSurfaceFrame],
        capsules: &[ClothCapsule],
        motion: ClothMotion,
        now_ms: i32,
    ) -> Result<HashMap<usize, ClothOutput>, String> {
        if !self.config.enabled {
            return Err("cloth garment simulation requested while disabled".to_owned());
        }
        if surfaces.is_empty() {
            return Ok(HashMap::new());
        }
        if !motion.matrix().is_finite()
            || motion.matrix().determinant().abs() < 1.0e-6
            || !to_metres(motion.origin).is_finite()
        {
            return Err("invalid cloth render transform".to_owned());
        }
        for surface in surfaces {
            if !Self::is_cloth_surface_name(&surface.surface_name) {
                return Err(format!(
                    "{model_label}: surface {:?} is not an auto-cloth surface",
                    surface.surface_name
                ));
            }
            if surface.bind_positions.len() != surface.posed_positions.len()
                || surface.posed_positions.len() != surface.posed_normals.len()
                || surface.posed_positions.len() != surface.skin_transforms.len()
            {
                return Err(format!(
                    "{model_label}:{}: cloth vertex arrays disagree ({}/{}/{})",
                    surface.surface_name,
                    surface.bind_positions.len(),
                    surface.posed_positions.len(),
                    surface.posed_normals.len()
                ));
            }
        }

        let signature = garment_signature(surfaces);
        let key = ClothKey { entity_num };
        let rebuild = self.instances.get(&key).is_none_or(|instance| {
            instance.model_label != model_label
                || instance.lod_index != lod_index
                || instance.signature != signature
        });
        if rebuild {
            match build_instance(model_label, lod_index, surfaces, capsules, motion, now_ms) {
                Ok(instance) => {
                    self.instances.insert(key.clone(), instance);
                }
                Err(error) => {
                    let surface_names = surfaces
                        .iter()
                        .map(|surface| surface.surface_name.as_str())
                        .collect::<Vec<_>>()
                        .join(",");
                    let failure = format!("{model_label}:lod{lod_index}:{surface_names}:{error}");
                    if self.reported_failures.insert(failure) {
                        devprintln!(1, "CLOTH: {model_label} garment rejected: {error}");
                    }
                    return Err(error);
                }
            }
        }

        let flat_positions = flatten_positions(surfaces, |surface| &surface.posed_positions);
        let config = self.config;
        let instance = self
            .instances
            .get_mut(&key)
            .expect("cloth garment inserted above");
        instance.last_seen_ms = now_ms;

        let authored_targets = particle_targets(&instance.particle_members, &flat_positions);
        let mut targets = garment_targets(
            instance,
            surfaces,
            &authored_targets,
            config.animation_influence,
        );
        if config.body_collision {
            project_body_reference(
                &mut targets,
                &authored_targets,
                capsules,
                &instance.pinned,
                &instance.clearance_weights,
                config.body_clearance.clamp(0.0, 4.0) * METRES_PER_JKA_UNIT,
                instance.cloth.material().contact_radius,
            );
        }
        if let Some(reason) = reset_reason(instance, &targets, motion, now_ms) {
            devprintln!(
                1,
                "CLOTHDBG ent={} {} reset: {}",
                entity_num,
                instance.model_label,
                reason,
            );
            instance
                .cloth
                .set_positions(&instance.reference)
                .map_err(|error| format!("cloth teleport reset failed: {error}"))?;
            instance.accumulator = 0.0;
            instance.frame_targets.clone_from(&targets);
            instance
                .frame_authored_targets
                .clone_from(&authored_targets);
            instance.tick_targets.clone_from(&targets);
            instance.frame_motion = motion;
            instance.tick_motion = motion;
            instance.frame_capsules = capsules.to_vec();
            instance.drive_velocities = None;
            instance.turn_velocities = None;
            instance.last_report = None;
        }

        let delta_ms = now_ms.saturating_sub(instance.last_time_ms);
        instance.last_time_ms = now_ms;
        let mut frame_steps = 0u32;
        if delta_ms > 0 && delta_ms <= 250 {
            let old_accumulator = instance.accumulator;
            instance.accumulator += delta_ms as f32 * 0.001;
            let h = 1.0 / config.hz.clamp(15, 240) as f32;
            let max_steps = config.max_substeps.clamp(1, 8);
            while instance.accumulator + f32::EPSILON >= h && frame_steps < max_steps {
                // Interpolate each fixed tick instead of feeding the newest
                // pose into all catch-up steps (which creates jump impulses).
                let t = ((h * (frame_steps + 1) as f32 - old_accumulator)
                    / (delta_ms as f32 * 0.001))
                    .clamp(0.0, 1.0);
                let tick_targets = instance
                    .frame_targets
                    .iter()
                    .zip(&targets)
                    .map(|(old, new)| old.lerp(*new, t))
                    .collect::<Vec<_>>();
                let tick_motion = instance.frame_motion.interpolate(motion, t);

                let fit_targets = instance
                    .frame_authored_targets
                    .iter()
                    .zip(&authored_targets)
                    .map(|(old, new)| old.lerp(*new, t))
                    .collect::<Vec<_>>();
                instance.wind_time = now_ms as f32 * 0.001 - instance.accumulator + h;
                drive_secondary_motion(instance, &tick_targets, tick_motion, h, config)?;
                let tick_capsules = interpolate_capsules(&instance.frame_capsules, capsules, t);
                let mut contacts = CapsuleContactSource {
                    capsules: if config.body_collision {
                        &tick_capsules
                    } else {
                        &[]
                    },
                    pinned: &instance.pinned,
                    reference: &instance.reference,
                    targets: &tick_targets,
                    fit_targets: &fit_targets,
                    clearance_weights: &instance.clearance_weights,
                    contact_limits: &instance.contact_limits,
                    prepared: Vec::new(),
                    clearance: config.body_clearance.clamp(0.0, 4.0) * METRES_PER_JKA_UNIT,
                };
                let follow_targets = build_follow_targets(
                    &instance.cloth,
                    &instance.pinned,
                    &instance.follow_strengths,
                    &instance.reference,
                );
                let stable_positions = instance.cloth.positions().to_vec();
                let stable_velocities = instance.cloth.velocities().to_vec();
                let report = match instance.solver.step_with_contacts(
                    &mut instance.cloth,
                    h,
                    Vec3::ZERO,
                    &SolverSettings {
                        max_substep: h,
                        ..SolverSettings::default()
                    },
                    &follow_targets,
                    &mut contacts,
                ) {
                    Ok(report) => report,
                    Err(error) => {
                        if now_ms.saturating_sub(instance.last_debug_ms) >= DEBUG_INTERVAL_MS {
                            instance.last_debug_ms = now_ms;
                            devprintln!(
                                1,
                                "CLOTHDBG ent={} model={} SOLVER ERROR: {} (pins={} collision={})",
                                entity_num,
                                instance.model_label,
                                error,
                                instance.pinned.len(),
                                config.body_collision as u8,
                            );
                        }
                        return Err(format!("cloth solver failed: {error}"));
                    }
                };

                // Validate each native result before another substep or output.
                // Restore the last valid displacement rather than displaying a
                // stretched garment for a frame and resetting it afterwards.
                if instance
                    .cloth
                    .positions()
                    .iter()
                    .zip(&instance.reference)
                    .any(|(position, reference)| {
                        !position.is_finite()
                            || (*position - *reference).length_squared()
                                > RUNAWAY_DISTANCE_METRES.powi(2)
                    })
                {
                    instance.rejected_steps += 1;
                    instance
                        .cloth
                        .set_positions(&stable_positions)
                        .map_err(|error| format!("cloth invalid-step restore failed: {error}"))?;
                    for (i, velocity) in stable_velocities.into_iter().enumerate() {
                        instance
                            .cloth
                            .set_velocity(i as u32, velocity)
                            .map_err(|error| {
                                format!("cloth invalid-step velocity restore failed: {error}")
                            })?;
                    }
                    devprintln!(
                        1,
                        "CLOTH: {} rejected unstable collision step; previous valid state restored",
                        instance.model_label
                    );
                }

                // A malformed custom mesh should not be able to poison later
                // frames with absurd local-space velocity even when all values
                // remain finite. This is only a safety ceiling; normal cloth
                // motion is far below it.
                clamp_free_particle_speeds(&mut instance.cloth, &instance.pinned, 3.0)?;

                instance.last_report = Some(report);
                instance.accumulator -= h;
                frame_steps += 1;
            }
            if frame_steps == max_steps && instance.accumulator >= h {
                instance.accumulator %= h;
                // Discard excess elapsed time without turning it into a large
                // drive impulse on the next tick.
                instance.tick_targets.clone_from(&targets);
                instance.tick_motion = motion;
                instance.drive_velocities = None;
                instance.turn_velocities = None;
            }
        }

        if delta_ms > 0 {
            instance.frame_targets.clone_from(&targets);
            instance
                .frame_authored_targets
                .clone_from(&authored_targets);
            instance.frame_motion = motion;
            instance.frame_capsules = capsules.to_vec();
        }

        let max_deviation_sq = instance
            .cloth
            .positions()
            .iter()
            .zip(instance.reference.iter())
            .map(|(position, target)| {
                if !position.is_finite() {
                    f32::INFINITY
                } else {
                    (*position - *target).length_squared()
                }
            })
            .fold(0.0f32, f32::max);
        let runaway = max_deviation_sq > RUNAWAY_DISTANCE_METRES * RUNAWAY_DISTANCE_METRES;
        if runaway {
            devprintln!(
                1,
                "CLOTH: {} garment runaway ({:.1}u from animated pose); resetting",
                instance.model_label,
                max_deviation_sq.sqrt() / METRES_PER_JKA_UNIT,
            );
            instance
                .cloth
                .set_positions(&instance.reference)
                .map_err(|error| format!("cloth runaway reset failed: {error}"))?;
            for &particle in &instance.pinned {
                instance
                    .cloth
                    .pin(particle, instance.reference[particle as usize])
                    .map_err(|error| format!("cloth runaway pin restore failed: {error}"))?;
            }
            instance.accumulator = 0.0;
        }

        let last_report = instance.last_report.clone();
        maybe_print_debug(
            entity_num,
            instance,
            &targets,
            last_report.as_ref(),
            frame_steps,
            config.body_collision,
            now_ms,
        );

        let mut outputs = HashMap::with_capacity(surfaces.len());
        for (layout, surface) in instance.layouts.iter().zip(surfaces) {
            if layout.surface_index != surface.surface_index
                || layout.render_to_particle.len() != surface.posed_positions.len()
            {
                return Err("cloth garment surface layout changed without rebuild".to_owned());
            }
            let mut positions = Vec::with_capacity(surface.posed_positions.len());
            for (render_vertex, &particle) in layout.render_to_particle.iter().enumerate() {
                if particle == u32::MAX {
                    positions.push(surface.posed_positions[render_vertex]);
                    continue;
                }
                let index = particle as usize;
                let mut offset = instance.cloth.positions()[index] - instance.reference[index];
                // Keep authored attachment seams visually welded to Ghoul2 even
                // on render frames that arrive between fixed cloth ticks.
                if instance.pinned.binary_search(&particle).is_ok() {
                    offset = Vec3::ZERO;
                }
                // A seam weld is an exact shared render position, too. Adding
                // the offset to each original skinned copy reopens gaps when
                // the two surfaces have different authored bone weights.
                let p = from_metres(targets[index] + offset);
                positions.push(if p.iter().all(|v| v.is_finite()) {
                    p
                } else {
                    surface.posed_positions[render_vertex]
                });
            }
            let normals = recompute_normals(&positions, &surface.posed_normals, &surface.triangles);
            outputs.insert(surface.surface_index, ClothOutput { positions, normals });
        }
        smooth_garment_normals(instance, surfaces, &mut outputs);
        Ok(outputs)
    }
}

/// Recompute area-weighted normals across welded render surfaces. Use authored
/// normal angles to preserve intentional creases instead of smoothing all UV
/// and material seams indiscriminately.
fn smooth_garment_normals(
    instance: &ClothInstance,
    surfaces: &[ClothSurfaceFrame],
    outputs: &mut HashMap<usize, ClothOutput>,
) {
    let total: usize = surfaces
        .iter()
        .map(|surface| surface.posed_positions.len())
        .sum();
    let mut face_sums = vec![Vec3::ZERO; total];
    let mut authored = Vec::with_capacity(total);
    let mut offset = 0;
    for surface in surfaces {
        let output = &outputs[&surface.surface_index];
        authored.extend(
            surface
                .posed_normals
                .iter()
                .map(|normal| Vec3::from_array(*normal).normalize_or_zero()),
        );
        for &[a, c, b] in &surface.triangles {
            let (a, b, c) = (a as usize, b as usize, c as usize);
            let (Some(pa), Some(pb), Some(pc)) = (
                output.positions.get(a),
                output.positions.get(b),
                output.positions.get(c),
            ) else {
                continue;
            };
            let face = (Vec3::from_array(*pb) - Vec3::from_array(*pa))
                .cross(Vec3::from_array(*pc) - Vec3::from_array(*pa));
            if face.is_finite() && face.length_squared() > 1.0e-12 {
                face_sums[offset + a] += face;
                face_sums[offset + b] += face;
                face_sums[offset + c] += face;
            }
        }
        offset += surface.posed_positions.len();
    }
    offset = 0;
    for (layout, surface) in instance.layouts.iter().zip(surfaces) {
        let output = outputs
            .get_mut(&surface.surface_index)
            .expect("cloth output inserted");
        for (render, &particle) in layout.render_to_particle.iter().enumerate() {
            if particle == u32::MAX {
                continue;
            }
            let global = offset + render;
            let mut sum = Vec3::ZERO;
            for &member in &instance.particle_members[particle as usize] {
                if authored[global].dot(authored[member]) >= 0.5 {
                    sum += face_sums[member];
                }
            }
            if sum.length_squared() > 1.0e-12 {
                output.normals[render] = sum.normalize().to_array();
            }
        }
        offset += surface.posed_positions.len();
    }
}

type SkinTransform = [[f32; 4]; 3];

fn compute_attachment_drivers(
    positions: &[Vec3],
    triangles: &[[u32; 3]],
    pinned: &[u32],
) -> (Vec<u32>, Vec<Vec<(usize, f32)>>) {
    let mut anchors = pinned.to_vec();
    // A separate cloth component can still use a soft attachment reference if
    // the sparse hard-anchor selection did not choose any particle in it.
    for component in cloth_components(positions.len(), triangles) {
        if !component
            .iter()
            .any(|&p| pinned.binary_search(&(p as u32)).is_ok())
        {
            if let Some(&top) = component
                .iter()
                .max_by(|&&a, &&b| positions[a].z.total_cmp(&positions[b].z))
            {
                anchors.push(top as u32);
            }
        }
    }
    anchors.sort_unstable();
    anchors.dedup();
    let mut adjacency = vec![Vec::<(usize, f32)>::new(); positions.len()];
    for (a, b, length) in build_debug_edges(positions, triangles) {
        adjacency[a as usize].push((b as usize, length));
        adjacency[b as usize].push((a as usize, length));
    }
    let distances = anchors
        .iter()
        .map(|&anchor| {
            let mut distances = vec![f32::INFINITY; positions.len()];
            let mut queue = VecDeque::from([anchor as usize]);
            let mut queued = vec![false; positions.len()];
            distances[anchor as usize] = 0.0;
            queued[anchor as usize] = true;
            while let Some(vertex) = queue.pop_front() {
                queued[vertex] = false;
                for &(next, length) in &adjacency[vertex] {
                    let distance = distances[vertex] + length;
                    if distance + 1.0e-6 < distances[next] {
                        distances[next] = distance;
                        if !queued[next] {
                            queue.push_back(next);
                            queued[next] = true;
                        }
                    }
                }
            }
            distances
        })
        .collect::<Vec<_>>();
    let drivers = (0..positions.len())
        .map(|particle| {
            let mut nearby = distances
                .iter()
                .enumerate()
                .filter_map(|(anchor, distances)| {
                    distances[particle]
                        .is_finite()
                        .then_some((anchor, distances[particle]))
                })
                .collect::<Vec<_>>();
            nearby.sort_by(|a, b| a.1.total_cmp(&b.1));
            nearby.truncate(3);
            if nearby
                .first()
                .is_some_and(|(_, distance)| *distance < 1.0e-6)
            {
                return vec![(nearby[0].0, 1.0)];
            }
            let sum: f32 = nearby
                .iter()
                .map(|(_, distance)| 1.0 / (distance + 0.025).powi(2))
                .sum();
            nearby
                .into_iter()
                .map(|(anchor, distance)| (anchor, 1.0 / (distance + 0.025).powi(2) / sum))
                .collect()
        })
        .collect();
    (anchors, drivers)
}

/// Blend free fabric toward the skin transforms of its nearest sewn attachments.
/// Distance along the mesh determines independence; bone names and animation
/// events are irrelevant. Attachments retain their exact authored positions.
fn garment_targets(
    instance: &ClothInstance,
    surfaces: &[ClothSurfaceFrame],
    authored: &[Vec3],
    influence: f32,
) -> Vec<Vec3> {
    if influence >= 1.0 {
        return authored.to_vec();
    }
    let flat = surfaces
        .iter()
        .flat_map(|surface| surface.skin_transforms.iter())
        .collect::<Vec<_>>();
    let transforms = instance
        .driver_particles
        .iter()
        .map(|&particle| {
            let members = &instance.particle_members[particle as usize];
            let mut matrix = [[0.0; 4]; 3];
            for &render in members {
                for row in 0..3 {
                    for column in 0..4 {
                        matrix[row][column] += flat[render][row][column] / members.len() as f32;
                    }
                }
            }
            matrix
        })
        .collect::<Vec<SkinTransform>>();
    authored
        .iter()
        .enumerate()
        .map(|(particle, authored)| {
            let freedom = instance.animation_freedom[particle] * (1.0 - influence.clamp(0.0, 1.0));
            if freedom <= 0.0 {
                return *authored;
            }
            let bind = instance.bind_reference[particle];
            let mut attached = Vec3::ZERO;
            for &(driver, weight) in &instance.attachment_drivers[particle] {
                let matrix = &transforms[driver];
                let point = Vec3::new(
                    matrix[0][0] * bind.x
                        + matrix[0][1] * bind.y
                        + matrix[0][2] * bind.z
                        + matrix[0][3] * METRES_PER_JKA_UNIT,
                    matrix[1][0] * bind.x
                        + matrix[1][1] * bind.y
                        + matrix[1][2] * bind.z
                        + matrix[1][3] * METRES_PER_JKA_UNIT,
                    matrix[2][0] * bind.x
                        + matrix[2][1] * bind.y
                        + matrix[2][2] * bind.z
                        + matrix[2][3] * METRES_PER_JKA_UNIT,
                );
                attached += point * weight;
            }
            authored.lerp(attached, freedom)
        })
        .collect()
}

fn garment_signature(surfaces: &[ClothSurfaceFrame]) -> Vec<SurfaceSignature> {
    surfaces
        .iter()
        .map(|surface| SurfaceSignature {
            surface_index: surface.surface_index,
            surface_name: surface.surface_name.clone(),
            vertex_count: surface.bind_positions.len(),
            triangle_count: surface.triangles.len(),
        })
        .collect()
}

fn flatten_positions<'a>(
    surfaces: &'a [ClothSurfaceFrame],
    select: impl Fn(&'a ClothSurfaceFrame) -> &'a [[f32; 3]],
) -> Vec<[f32; 3]> {
    let total = surfaces.iter().map(|surface| select(surface).len()).sum();
    let mut out = Vec::with_capacity(total);
    for surface in surfaces {
        out.extend_from_slice(select(surface));
    }
    out
}

#[derive(Debug, Clone, Copy)]
struct BoundaryNode {
    root: usize,
    surface_ordinal: usize,
    position: Vec3,
}

#[derive(Debug, Clone, Copy)]
struct SeamCandidate {
    a: BoundaryNode,
    b: BoundaryNode,
    distance_sq: f32,
}

struct DisjointSet {
    parent: Vec<usize>,
    rank: Vec<u8>,
}

impl DisjointSet {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
            rank: vec![0; len],
        }
    }

    fn find(&mut self, value: usize) -> usize {
        let parent = self.parent[value];
        if parent != value {
            let root = self.find(parent);
            self.parent[value] = root;
        }
        self.parent[value]
    }

    fn union(&mut self, a: usize, b: usize) -> bool {
        let mut a = self.find(a);
        let mut b = self.find(b);
        if a == b {
            return false;
        }
        if self.rank[a] < self.rank[b] {
            std::mem::swap(&mut a, &mut b);
        }
        self.parent[b] = a;
        if self.rank[a] == self.rank[b] {
            self.rank[a] = self.rank[a].saturating_add(1);
        }
        true
    }
}

fn build_instance(
    model_label: &str,
    lod_index: usize,
    surfaces: &[ClothSurfaceFrame],
    capsules: &[ClothCapsule],
    motion: ClothMotion,
    now_ms: i32,
) -> Result<ClothInstance, String> {
    let total_vertices: usize = surfaces
        .iter()
        .map(|surface| surface.bind_positions.len())
        .sum();
    if total_vertices < 3 {
        return Err("not enough garment geometry".to_owned());
    }

    let mut offsets = Vec::with_capacity(surfaces.len());
    let mut cursor = 0usize;
    for surface in surfaces {
        if surface.bind_positions.len() < 3 || surface.triangles.is_empty() {
            return Err(format!(
                "surface {:?} has insufficient geometry",
                surface.surface_name
            ));
        }
        offsets.push(cursor);
        cursor += surface.bind_positions.len();
    }
    let flat_posed = flatten_positions(surfaces, |surface| &surface.posed_positions);
    let flat_bind = flatten_positions(surfaces, |surface| &surface.bind_positions);
    let mut dsu = DisjointSet::new(total_vertices);

    // First remove ordinary UV/normal duplication *within each GLM surface*.
    // Include surface ordinal in the key so close pieces are only cross-stitched
    // by the controlled boundary pass below.
    let mut intra_weld = HashMap::<(usize, i32, i32, i32), usize>::new();
    for (surface_ordinal, surface) in surfaces.iter().enumerate() {
        let offset = offsets[surface_ordinal];
        for (local_index, p) in surface.bind_positions.iter().enumerate() {
            let key = (
                surface_ordinal,
                (p[0] * INTRA_SURFACE_WELD_SCALE).round() as i32,
                (p[1] * INTRA_SURFACE_WELD_SCALE).round() as i32,
                (p[2] * INTRA_SURFACE_WELD_SCALE).round() as i32,
            );
            let global = offset + local_index;
            if let Some(&existing) = intra_weld.get(&key) {
                dsu.union(existing, global);
            } else {
                intra_weld.insert(key, global);
            }
        }
    }

    // Find physical boundary vertices after the tight intra-surface weld. Only
    // these are eligible for the intentionally loose cross-surface stitch.
    let mut boundary_nodes = Vec::new();
    for (surface_ordinal, surface) in surfaces.iter().enumerate() {
        boundary_nodes.extend(surface_boundary_nodes(
            surface,
            surface_ordinal,
            offsets[surface_ordinal],
            &mut dsu,
        ));
    }

    // Greedy nearest-neighbour seam matching. A boundary root can match at most
    // one root from a particular other surface, preventing a loose tolerance
    // from collapsing several adjacent vertices onto one point. It may still
    // match a third surface at a genuine three-way garment junction.
    let max_distance_sq = CROSS_SURFACE_WELD_DISTANCE_JKA * CROSS_SURFACE_WELD_DISTANCE_JKA;
    let mut candidates = Vec::new();
    for a_index in 0..boundary_nodes.len() {
        let a = boundary_nodes[a_index];
        for &b in &boundary_nodes[a_index + 1..] {
            if a.surface_ordinal == b.surface_ordinal {
                continue;
            }
            let distance_sq = (a.position - b.position).length_squared();
            if distance_sq <= max_distance_sq {
                candidates.push(SeamCandidate { a, b, distance_sq });
            }
        }
    }
    candidates.sort_by(|a, b| a.distance_sq.total_cmp(&b.distance_sq));
    let mut matched_against = HashSet::<(usize, usize)>::new();
    let mut seam_welds = 0usize;
    let mut seam_weld_distance_sum = 0.0f32;
    let mut seam_weld_distance_max = 0.0f32;
    for candidate in candidates {
        let a_key = (candidate.a.root, candidate.b.surface_ordinal);
        let b_key = (candidate.b.root, candidate.a.surface_ordinal);
        if matched_against.contains(&a_key) || matched_against.contains(&b_key) {
            continue;
        }
        if dsu.union(candidate.a.root, candidate.b.root) {
            seam_welds += 1;
            let distance = candidate.distance_sq.sqrt();
            seam_weld_distance_sum += distance;
            seam_weld_distance_max = seam_weld_distance_max.max(distance);
        }
        matched_against.insert(a_key);
        matched_against.insert(b_key);
    }

    // Build one triangle soup over the now-unified physical particles. Render
    // surfaces remain separate; only their topology is shared here.
    let mut root_triangles = Vec::<[usize; 3]>::new();
    let mut canonical_roots = HashSet::<[usize; 3]>::new();
    for (surface_ordinal, surface) in surfaces.iter().enumerate() {
        let offset = offsets[surface_ordinal];
        for triangle in &surface.triangles {
            let Some(a_local) = valid_index(triangle[0], surface.bind_positions.len()) else {
                continue;
            };
            let Some(b_local) = valid_index(triangle[1], surface.bind_positions.len()) else {
                continue;
            };
            let Some(c_local) = valid_index(triangle[2], surface.bind_positions.len()) else {
                continue;
            };
            let a = dsu.find(offset + a_local);
            let b = dsu.find(offset + b_local);
            let c = dsu.find(offset + c_local);
            if a == b || b == c || c == a {
                continue;
            }
            let mut signature = [a, b, c];
            signature.sort_unstable();
            if canonical_roots.insert(signature) {
                root_triangles.push([a, b, c]);
            }
        }
    }
    if root_triangles.is_empty() {
        return Err("all garment triangles became degenerate after seam welding".to_owned());
    }

    let mut used_roots = HashSet::<usize>::new();
    for triangle in &root_triangles {
        used_roots.extend(triangle.iter().copied());
    }
    let mut roots = used_roots.into_iter().collect::<Vec<_>>();
    roots.sort_unstable();
    let root_to_particle = roots
        .iter()
        .enumerate()
        .map(|(particle, &root)| (root, particle as u32))
        .collect::<HashMap<_, _>>();

    let mut particle_members = vec![Vec::<usize>::new(); roots.len()];
    let mut global_render_to_particle = vec![u32::MAX; total_vertices];
    for global in 0..total_vertices {
        let root = dsu.find(global);
        let Some(&particle) = root_to_particle.get(&root) else {
            continue;
        };
        global_render_to_particle[global] = particle;
        particle_members[particle as usize].push(global);
    }

    let sim_triangles = root_triangles
        .into_iter()
        .map(|triangle| {
            [
                root_to_particle[&triangle[0]],
                root_to_particle[&triangle[1]],
                root_to_particle[&triangle[2]],
            ]
        })
        .collect::<Vec<_>>();

    let layouts = surfaces
        .iter()
        .enumerate()
        .map(|(surface_ordinal, surface)| {
            let offset = offsets[surface_ordinal];
            let render_to_particle = (0..surface.bind_positions.len())
                .map(|local| {
                    let particle = global_render_to_particle[offset + local];
                    particle
                })
                .collect();
            ClothSurfaceLayout {
                surface_index: surface.surface_index,
                render_to_particle,
            }
        })
        .collect::<Vec<_>>();

    let rest_positions = particle_targets(&particle_members, &flat_posed);
    let mesh = ClothMesh::new(rest_positions.clone(), sim_triangles.clone())
        .map_err(|error| format!("invalid stitched cloth topology: {error}"))?;
    let mut material = ClothMaterial::default();
    // The library default is intentionally very lightly damped. Character
    // clothing is driven every frame by an animated skeleton, so stronger
    // damping is appropriate and prevents small animation/contact corrections
    // from ringing for seconds.
    material.damping = 8.0;
    material.bend_compliance = 5.0e-3;
    let mut cloth = Cloth::new(mesh, material)
        .map_err(|error| format!("could not create stitched cloth: {error}"))?;

    // One top band for an entire stitched robe is wrong: once hood + torso +
    // sleeves + skirt form one connected component, that anchors only the hood
    // and lets the rest of the garment hang from the character's head. The
    // opposite extreme (pinning every source-surface top row) made the garment
    // nearly rigid. Keep only a tiny, spatially-spread set of anchors per
    // authored cloth surface. This preserves the skeleton's structural joints
    // while leaving the fabric between them dynamic.
    let (mut pinned, pin_debug) =
        choose_structural_surface_anchors(surfaces, &offsets, &mut dsu, &root_to_particle);
    // Small closed openings around the body act as sewn cuffs/collars. Large
    // free hems remain dynamic. This uses topology and proximity, not names.
    let opening_pins = fitted_opening_anchors(&rest_positions, &sim_triangles, capsules);
    pinned.extend(opening_pins.iter().copied());
    pinned.sort_unstable();
    pinned.dedup();
    devprintln!(1, "CLOTHDBG fitted opening anchors={}", opening_pins.len());
    let component_count = cloth_components(rest_positions.len(), &sim_triangles).len();
    let mut follow_strengths = compute_follow_strengths(&rest_positions, &sim_triangles, &pinned);
    // Collision padding tapers into sewn attachments, which cannot move outward.
    let clearance_weights = follow_strengths
        .iter()
        .map(|strength| {
            ((FOLLOW_NEAR_STRENGTH - strength) / (FOLLOW_NEAR_STRENGTH - FOLLOW_FAR_STRENGTH))
                .clamp(0.0, 1.0)
                .sqrt()
        })
        .collect();
    for (layout, surface) in layouts.iter().zip(surfaces) {
        let name = surface.surface_name.to_ascii_lowercase();
        let minimum = if name.contains("hood") || name.contains("head") {
            0.16
        } else if name.contains("shoulder") || name.contains("torso") {
            0.12
        } else {
            0.0
        };
        for &particle in &layout.render_to_particle {
            if particle != u32::MAX && pinned.binary_search(&particle).is_err() {
                follow_strengths[particle as usize] =
                    follow_strengths[particle as usize].max(minimum);
            }
        }
    }
    let bind_reference = particle_targets(&particle_members, &flat_bind);
    let (driver_particles, attachment_drivers) =
        compute_attachment_drivers(&bind_reference, &sim_triangles, &pinned);
    let animation_freedom = follow_strengths
        .iter()
        .enumerate()
        .map(|(particle, strength)| {
            if pinned.binary_search(&(particle as u32)).is_ok() {
                0.0
            } else {
                ((FOLLOW_NEAR_STRENGTH - strength) / (FOLLOW_NEAR_STRENGTH - FOLLOW_FAR_STRENGTH))
                    .clamp(0.0, 1.0)
            }
        })
        .collect();
    let debug_edges = build_debug_edges(&rest_positions, &sim_triangles);
    for &particle in &pinned {
        cloth
            .pin(particle, rest_positions[particle as usize])
            .map_err(|error| format!("could not pin garment attachment vertex: {error}"))?;
    }

    let surface_names = surfaces
        .iter()
        .map(|surface| surface.surface_name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    devprintln!(
        1,
        "CLOTH: {model_label} garment LOD {lod_index} [{}] -> {} surfaces / {} particles / {} triangles / {} component(s) / {} structural pins / {} cross-surface welds (avg/max {:.3}/{:.3}u, limit {:.2}u) / authored secondary offsets",
        surface_names,
        surfaces.len(),
        rest_positions.len(),
        sim_triangles.len(),
        component_count,
        pinned.len(),
        seam_welds,
        if seam_welds > 0 { seam_weld_distance_sum / seam_welds as f32 } else { 0.0 },
        seam_weld_distance_max,
        CROSS_SURFACE_WELD_DISTANCE_JKA,
    );
    for (surface_name, count) in pin_debug {
        devprintln!(1, "CLOTHDBG anchor surface={} pins={}", surface_name, count);
    }

    Ok(ClothInstance {
        model_label: model_label.to_owned(),
        lod_index,
        signature: garment_signature(surfaces),
        cloth,
        solver: Solver::new(),
        particle_members,
        layouts,
        pinned,
        follow_strengths,
        reference: rest_positions.clone(),
        bind_reference,
        driver_particles,
        attachment_drivers,
        animation_freedom,
        clearance_weights,
        contact_limits: {
            let mut lengths = vec![f32::INFINITY; rest_positions.len()];
            for &[a, b, c] in &sim_triangles {
                for (a, b) in [(a, b), (b, c), (c, a)] {
                    let length = rest_positions[a as usize].distance(rest_positions[b as usize]);
                    lengths[a as usize] = lengths[a as usize].min(length);
                    lengths[b as usize] = lengths[b as usize].min(length);
                }
            }
            lengths
                .into_iter()
                .map(|length| (length * 0.2).clamp(0.0015, 0.01))
                .collect()
        },
        frame_targets: rest_positions.clone(),
        frame_authored_targets: rest_positions.clone(),
        wind_noise: {
            let mut noise = fastnoise_lite::FastNoiseLite::with_seed(1937);
            noise.set_noise_type(Some(fastnoise_lite::NoiseType::Perlin));
            noise.set_frequency(Some(1.0));
            noise
        },
        wind_time: now_ms as f32 * 0.001,
        frame_motion: motion,
        frame_capsules: capsules.to_vec(),
        tick_targets: rest_positions,
        tick_motion: motion,
        drive_velocities: None,
        turn_velocities: None,
        debug_edges,
        accumulator: 0.0,
        last_time_ms: now_ms,
        last_seen_ms: now_ms,
        last_debug_ms: now_ms.saturating_sub(DEBUG_INTERVAL_MS),
        last_report: None,
        rejected_steps: 0,
    })
}

#[inline]
fn valid_index(index: u32, len: usize) -> Option<usize> {
    let index = index as usize;
    (index < len).then_some(index)
}

fn surface_boundary_nodes(
    surface: &ClothSurfaceFrame,
    surface_ordinal: usize,
    offset: usize,
    dsu: &mut DisjointSet,
) -> Vec<BoundaryNode> {
    let mut edge_count = HashMap::<(usize, usize), u32>::new();
    for triangle in &surface.triangles {
        let Some(a) = valid_index(triangle[0], surface.bind_positions.len()) else {
            continue;
        };
        let Some(b) = valid_index(triangle[1], surface.bind_positions.len()) else {
            continue;
        };
        let Some(c) = valid_index(triangle[2], surface.bind_positions.len()) else {
            continue;
        };
        let roots = [
            dsu.find(offset + a),
            dsu.find(offset + b),
            dsu.find(offset + c),
        ];
        if roots[0] == roots[1] || roots[1] == roots[2] || roots[2] == roots[0] {
            continue;
        }
        for (a, b) in [
            (roots[0], roots[1]),
            (roots[1], roots[2]),
            (roots[2], roots[0]),
        ] {
            let edge = if a < b { (a, b) } else { (b, a) };
            *edge_count.entry(edge).or_default() += 1;
        }
    }

    let mut boundary_roots = HashSet::<usize>::new();
    for ((a, b), count) in edge_count {
        if count == 1 {
            boundary_roots.insert(a);
            boundary_roots.insert(b);
        }
    }
    if boundary_roots.is_empty() {
        return Vec::new();
    }

    let mut sums = HashMap::<usize, (Vec3, f32)>::new();
    for (local_vertex, &position) in surface.bind_positions.iter().enumerate() {
        let root = dsu.find(offset + local_vertex);
        if !boundary_roots.contains(&root) {
            continue;
        }
        let entry = sums.entry(root).or_insert((Vec3::ZERO, 0.0));
        entry.0 += Vec3::from_array(position);
        entry.1 += 1.0;
    }
    let mut nodes = sums
        .into_iter()
        .filter_map(|(root, (sum, count))| {
            (count > 0.0).then_some(BoundaryNode {
                root,
                surface_ordinal,
                position: sum / count,
            })
        })
        .collect::<Vec<_>>();
    nodes.sort_by_key(|node| node.root);
    nodes
}

fn particle_targets(members: &[Vec<usize>], render_positions: &[[f32; 3]]) -> Vec<Vec3> {
    members
        .iter()
        .map(|members| {
            let mut sum = Vec3::ZERO;
            let mut count = 0.0f32;
            for &render_vertex in members {
                if let Some(&p) = render_positions.get(render_vertex) {
                    sum += to_metres(p);
                    count += 1.0;
                }
            }
            if count > 0.0 {
                sum / count
            } else {
                Vec3::ZERO
            }
        })
        .collect()
}

fn cloth_components(particle_count: usize, triangles: &[[u32; 3]]) -> Vec<Vec<usize>> {
    let mut adjacency = vec![Vec::<usize>::new(); particle_count];
    for triangle in triangles {
        let vertices = [
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        ];
        for (a, b) in [
            (vertices[0], vertices[1]),
            (vertices[1], vertices[2]),
            (vertices[2], vertices[0]),
        ] {
            if a >= particle_count || b >= particle_count || a == b {
                continue;
            }
            adjacency[a].push(b);
            adjacency[b].push(a);
        }
    }

    let mut visited = vec![false; particle_count];
    let mut components = Vec::new();
    for start in 0..particle_count {
        if visited[start] {
            continue;
        }
        visited[start] = true;
        let mut queue = VecDeque::new();
        queue.push_back(start);
        let mut component = Vec::new();
        while let Some(vertex) = queue.pop_front() {
            component.push(vertex);
            for &next in &adjacency[vertex] {
                if !visited[next] {
                    visited[next] = true;
                    queue.push_back(next);
                }
            }
        }
        components.push(component);
    }
    components
}

/// Select a very small set of *hard* skeleton attachments from structurally
/// stable authored surfaces. Hard-pinning the top of every source surface looks
/// tempting after stitching, but it over-constrains a skinned model: an arm or
/// hood pin follows a different animated bone while XPBD simultaneously insists
/// that all sewn edge rest lengths stay fixed. Saber/torso animation can then
/// make those constraints mutually impossible and the solver visibly spasms.
///
/// Prefer central body sections (hips/pelvis/waist and torso/chest/back). Limb,
/// shoulder and head/hood pieces are driven by the soft solver-native targets
/// instead. If a custom cape/robe has no recognizable central surface, fall
/// back to one best generic cloth surface so the garment cannot free-fall.
fn choose_structural_surface_anchors(
    surfaces: &[ClothSurfaceFrame],
    offsets: &[usize],
    dsu: &mut DisjointSet,
    root_to_particle: &HashMap<usize, u32>,
) -> (Vec<u32>, Vec<(String, usize)>) {
    fn anchor_priority(name: &str) -> u8 {
        let name = name.to_ascii_lowercase();
        // Independently animated appendages always win over generic body words
        // in compound surface names (for example torso_robe_hood).
        if [
            "arm", "shoulder", "hand", "leg", "head", "hood", "sleeve", "forearm",
        ]
        .iter()
        .any(|needle| name.contains(needle))
        {
            return 0;
        }
        if ["hips", "hip_", "pelvis", "waist", "belt"]
            .iter()
            .any(|needle| name.contains(needle))
        {
            return 3;
        }
        if ["torso", "chest", "spine", "back"]
            .iter()
            .any(|needle| name.contains(needle))
        {
            return 2;
        }
        1
    }

    let priorities = surfaces
        .iter()
        .map(|surface| anchor_priority(&surface.surface_name))
        .collect::<Vec<_>>();
    let has_central = priorities.iter().any(|&priority| priority >= 2);
    let fallback = if has_central {
        None
    } else {
        priorities
            .iter()
            .enumerate()
            .max_by_key(|(_, priority)| **priority)
            .map(|(index, _)| index)
    };

    let mut pinned = Vec::<u32>::new();
    let mut debug = Vec::with_capacity(surfaces.len());
    for (surface_ordinal, surface) in surfaces.iter().enumerate() {
        let priority = priorities[surface_ordinal];
        let use_surface = if has_central {
            priority >= 2
        } else {
            fallback == Some(surface_ordinal)
        };
        if !use_surface || surface.bind_positions.is_empty() {
            debug.push((surface.surface_name.clone(), 0));
            continue;
        }

        let min_z = surface
            .bind_positions
            .iter()
            .map(|p| p[2])
            .fold(f32::INFINITY, f32::min);
        let max_z = surface
            .bind_positions
            .iter()
            .map(|p| p[2])
            .fold(f32::NEG_INFINITY, f32::max);
        let height = (max_z - min_z).max(1.0);
        let band = (height * 0.08).clamp(0.5, 3.0);
        let offset = offsets[surface_ordinal];

        let mut candidate_sums = HashMap::<u32, (Vec3, f32)>::new();
        for (local, &position) in surface.bind_positions.iter().enumerate() {
            if position[2] < max_z - band {
                continue;
            }
            let root = dsu.find(offset + local);
            let Some(&particle) = root_to_particle.get(&root) else {
                continue;
            };
            let entry = candidate_sums.entry(particle).or_insert((Vec3::ZERO, 0.0));
            entry.0 += Vec3::from_array(position);
            entry.1 += 1.0;
        }
        if candidate_sums.is_empty() {
            for (local, &position) in surface.bind_positions.iter().enumerate() {
                let root = dsu.find(offset + local);
                let Some(&particle) = root_to_particle.get(&root) else {
                    continue;
                };
                let entry = candidate_sums.entry(particle).or_insert((Vec3::ZERO, 0.0));
                entry.0 += Vec3::from_array(position);
                entry.1 += 1.0;
            }
        }
        let mut candidates = candidate_sums
            .into_iter()
            .filter_map(|(particle, (sum, count))| (count > 0.0).then_some((particle, sum / count)))
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(particle, _)| *particle);

        let chosen = choose_spread_anchors(&candidates);
        debug.push((surface.surface_name.clone(), chosen.len()));
        pinned.extend(chosen);
    }

    pinned.sort_unstable();
    pinned.dedup();
    (pinned, debug)
}

fn choose_spread_anchors(candidates: &[(u32, Vec3)]) -> Vec<u32> {
    match candidates.len() {
        0 => return Vec::new(),
        1..=3 => return candidates.iter().map(|(particle, _)| *particle).collect(),
        _ => {}
    }

    let mut best_pair = (0usize, 1usize, -1.0f32);
    for a in 0..candidates.len() {
        for b in a + 1..candidates.len() {
            let pa = candidates[a].1;
            let pb = candidates[b].1;
            // Attachment bands are selected by Z, so spread them around the
            // horizontal ring rather than selecting three adjacent vertices.
            let distance_sq = (pa.x - pb.x).powi(2) + (pa.y - pb.y).powi(2);
            if distance_sq > best_pair.2 {
                best_pair = (a, b, distance_sq);
            }
        }
    }

    let mut chosen = vec![candidates[best_pair.0].0, candidates[best_pair.1].0];
    let pa = candidates[best_pair.0].1;
    let pb = candidates[best_pair.1].1;
    let midpoint = (pa + pb) * 0.5;
    if let Some((particle, _)) = candidates
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != best_pair.0 && *index != best_pair.1)
        .map(|(_, &(particle, position))| {
            let delta = position - midpoint;
            (particle, delta.x * delta.x + delta.y * delta.y)
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
    {
        chosen.push(particle);
    }
    chosen
}

fn build_debug_edges(rest_positions: &[Vec3], triangles: &[[u32; 3]]) -> Vec<(u32, u32, f32)> {
    let mut seen = HashSet::<(u32, u32)>::new();
    let mut edges = Vec::new();
    for triangle in triangles {
        for (a, b) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ] {
            let key = if a < b { (a, b) } else { (b, a) };
            if !seen.insert(key) {
                continue;
            }
            let Some(pa) = rest_positions.get(key.0 as usize) else {
                continue;
            };
            let Some(pb) = rest_positions.get(key.1 as usize) else {
                continue;
            };
            let length = pa.distance(*pb);
            if length.is_finite() && length > 1.0e-6 {
                edges.push((key.0, key.1, length));
            }
        }
    }
    edges
}

/// Desired fraction of animation error corrected by a solver substep. The
/// value fades along the *cloth topology* away from sparse skeleton anchors.
fn compute_follow_strengths(
    rest_positions: &[Vec3],
    triangles: &[[u32; 3]],
    pinned: &[u32],
) -> Vec<f32> {
    let count = rest_positions.len();
    let mut adjacency = vec![Vec::<(usize, f32)>::new(); count];
    for triangle in triangles {
        let vertices = [
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        ];
        for (a, b) in [
            (vertices[0], vertices[1]),
            (vertices[1], vertices[2]),
            (vertices[2], vertices[0]),
        ] {
            if a >= count || b >= count || a == b {
                continue;
            }
            let length = (rest_positions[a] - rest_positions[b]).length().max(1.0e-5);
            adjacency[a].push((b, length));
            adjacency[b].push((a, length));
        }
    }

    let mut distance = vec![f32::INFINITY; count];
    let mut queue = VecDeque::<usize>::new();
    let mut queued = vec![false; count];
    for &particle in pinned {
        let index = particle as usize;
        if index < count && distance[index] != 0.0 {
            distance[index] = 0.0;
            queue.push_back(index);
            queued[index] = true;
        }
    }
    while let Some(vertex) = queue.pop_front() {
        queued[vertex] = false;
        let base = distance[vertex];
        for &(next, edge_length) in &adjacency[vertex] {
            let candidate = base + edge_length;
            if candidate + 1.0e-6 < distance[next] {
                distance[next] = candidate;
                if !queued[next] {
                    queue.push_back(next);
                    queued[next] = true;
                }
            }
        }
    }

    distance
        .into_iter()
        .enumerate()
        .map(|(index, distance_metres)| {
            if pinned.binary_search(&(index as u32)).is_ok() {
                return 0.0;
            }
            let distance_jka = if distance_metres.is_finite() {
                distance_metres / METRES_PER_JKA_UNIT
            } else {
                FOLLOW_FALLOFF_JKA
            };
            let t = (distance_jka / FOLLOW_FALLOFF_JKA).clamp(0.0, 1.0);
            let smooth = t * t * (3.0 - 2.0 * t);
            FOLLOW_NEAR_STRENGTH * (1.0 - smooth) + FOLLOW_FAR_STRENGTH * smooth
        })
        .collect()
}

/// Convert a desired correction fraction at 60Hz into fixed XPBD compliance.
/// For an isolated target the solver applies `w / (w + compliance / h^2)` of
/// the error, where `w = 1/mass`. Compliance uses physical mass and a reference
/// timestep, so density and configured simulation rate do not change stiffness.
fn build_follow_targets(
    cloth: &Cloth,
    pinned: &[u32],
    follow_strengths: &[f32],
    positions: &[Vec3],
) -> Vec<Target> {
    if follow_strengths.len() != positions.len() || cloth.masses().len() != positions.len() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(positions.len().saturating_sub(pinned.len()));
    for index in 0..positions.len() {
        if pinned.binary_search(&(index as u32)).is_ok() {
            continue;
        }
        let strength = follow_strengths[index].clamp(0.0001, 0.95);
        let mass = cloth.masses()[index];
        if !mass.is_finite() || mass <= 0.0 {
            continue;
        }
        let inverse_mass = 1.0 / mass;
        // Calibrate at 60Hz and keep physical compliance fixed at other rates.
        // Multiplying by the current h squared weakens attachment sixteenfold at
        // 15Hz, making identical jump impulses produce much larger excursions.
        let reference_h = 1.0 / 60.0;
        let compliance = reference_h * reference_h * inverse_mass * (1.0 - strength) / strength;
        out.push(Target {
            particle: index as u32,
            position: positions[index],
            compliance,
        });
    }
    out
}

fn clamp_free_particle_speeds(
    cloth: &mut Cloth,
    pinned: &[u32],
    max_speed_metres: f32,
) -> Result<(), String> {
    let max_speed_sq = max_speed_metres * max_speed_metres;
    let velocities = cloth.velocities().to_vec();
    for (index, mut velocity) in velocities.into_iter().enumerate() {
        if pinned.binary_search(&(index as u32)).is_ok() {
            continue;
        }
        let speed_sq = velocity.length_squared();
        if speed_sq > max_speed_sq && speed_sq.is_finite() {
            velocity *= max_speed_metres / speed_sq.sqrt();
            cloth
                .set_velocity(index as u32, velocity)
                .map_err(|error| format!("cloth safety velocity clamp failed: {error}"))?;
        }
    }
    Ok(())
}

fn solver_vector(matrix: glam::Mat3, value: Vec3) -> Vec3 {
    Vec3::from_array((matrix * glam::Vec3::from_array(value.to_array())).to_array())
}

/// Pressure from relative airflow, integrated over the current garment faces.
/// Normal pressure is quadratic in speed; a smaller tangential term accounts
/// for drag along the fabric. Forces are shared by face area, not vertex count.
fn aerodynamic_forces(
    instance: &ClothInstance,
    targets: &[Vec3],
    motion: ClothMotion,
    world_velocities: &[Vec3],
    wind: Vec3,
    resistance: f32,
) -> Vec<Vec3> {
    let mut forces = vec![Vec3::ZERO; targets.len()];
    if resistance <= 0.0 {
        return forces;
    }
    let matrix = motion.matrix();
    let points = targets
        .iter()
        .enumerate()
        .map(|(i, target)| {
            solver_vector(
                matrix,
                *target + instance.cloth.positions()[i] - instance.reference[i],
            )
        })
        .collect::<Vec<_>>();
    for &[a, b, c] in instance.cloth.mesh().triangles() {
        let (a, b, c) = (a as usize, b as usize, c as usize);
        let cross = (points[b] - points[a]).cross(points[c] - points[a]);
        let twice_area = cross.length();
        if !twice_area.is_finite() || twice_area < 1.0e-9 {
            continue;
        }
        let normal = cross / twice_area;
        let relative = (wind
            - (world_velocities[a] + world_velocities[b] + world_velocities[c]) / 3.0)
            .clamp_length_max(30.0);
        // A continuous field advected in world space gives small eddies, with
        // no random per-frame impulses. Still air exerts no force.
        let center = (points[a] + points[b] + points[c]) / 3.0 + to_metres(motion.origin);
        let time = instance.wind_time * 4.0;
        let sample = |shift: f32| {
            instance.wind_noise.get_noise_3d(
                center.x * 7.0 + time + shift,
                center.y * 7.0 - time * 0.73,
                center.z * 7.0 + time * 0.41,
            )
        };
        let eddy = Vec3::new(sample(0.0), sample(31.7), sample(67.3));
        let relative = relative * (1.0 + eddy.x * 0.06) + relative.cross(eddy) * 0.04;
        let normal_speed = relative.dot(normal);
        let tangent = relative - normal * normal_speed;
        let force = (normal * (normal_speed * normal_speed.abs() * 1.2)
            + tangent * (relative.length() * 0.12))
            * (0.5 * 1.225 * twice_area * 0.5 * resistance / 3.0);
        forces[a] += force;
        forces[b] += force;
        forces[c] += force;
    }
    forces
}

/// Sample translation, turning, and animated vertex motion at fixed ticks.
/// State transport preserves secondary displacement and velocity during turns.
fn drive_secondary_motion(
    instance: &mut ClothInstance,
    targets: &[Vec3],
    motion: ClothMotion,
    h: f32,
    config: ClothConfig,
) -> Result<(), String> {
    let old_matrix = instance.tick_motion.matrix();
    let matrix = motion.matrix();
    let inverse = matrix.inverse();
    let transport = inverse * old_matrix;
    let old_velocities = instance.cloth.velocities().to_vec();
    let positions = instance
        .cloth
        .positions()
        .iter()
        .zip(&instance.reference)
        .map(|(position, reference)| *reference + solver_vector(transport, *position - *reference))
        .collect::<Vec<_>>();
    instance
        .cloth
        .set_positions(&positions)
        .map_err(|error| format!("cloth frame transport failed: {error}"))?;
    let root_velocity = (to_metres(motion.origin) - to_metres(instance.tick_motion.origin)) / h;
    let smoothing = 1.0 - (-h / 0.025).exp();
    let mut drive_velocities = Vec::with_capacity(targets.len());
    let mut turn_velocities = Vec::with_capacity(targets.len());
    let mut accelerations = Vec::with_capacity(targets.len());
    let mut world_velocities = Vec::with_capacity(targets.len());
    for index in 0..targets.len() {
        let velocity = root_velocity
            + (solver_vector(matrix, targets[index])
                - solver_vector(old_matrix, instance.tick_targets[index]))
                / h;
        let turn_velocity =
            (solver_vector(matrix, targets[index]) - solver_vector(old_matrix, targets[index])) / h;
        let velocity = instance
            .drive_velocities
            .as_ref()
            .map(|old| old[index].lerp(velocity, smoothing))
            .unwrap_or(velocity);
        let turn_velocity = instance
            .turn_velocities
            .as_ref()
            .map(|old| old[index].lerp(turn_velocity, smoothing))
            .unwrap_or(turn_velocity);
        let acceleration = instance
            .drive_velocities
            .as_ref()
            .map(|old| (velocity - old[index]) / h)
            .unwrap_or(Vec3::ZERO);
        let turn_acceleration = instance
            .turn_velocities
            .as_ref()
            .map(|old| (turn_velocity - old[index]) / h)
            .unwrap_or(Vec3::ZERO);
        let acceleration = (acceleration + turn_acceleration * (config.turn_response - 1.0))
            .clamp_length_max(MAX_DRIVE_ACCELERATION * 1.5);
        accelerations.push(acceleration);
        world_velocities.push(velocity + solver_vector(old_matrix, old_velocities[index]));
        drive_velocities.push(velocity);
        turn_velocities.push(turn_velocity);
    }
    let wind = to_metres(config.wind_velocity);
    let air_forces = aerodynamic_forces(
        instance,
        targets,
        motion,
        &world_velocities,
        wind,
        config.air_resistance.clamp(0.0, 4.0),
    );
    for index in 0..targets.len() {
        let mass = instance.cloth.masses()[index];
        // Bound aerodynamic impulses to avoid reversing relative flow in one
        // low-frequency tick or injecting huge gust energy into tiny triangles.
        let air_limit = ((wind - world_velocities[index]).length() / h).min(40.0);
        let air_acceleration = (air_forces[index] / mass).clamp_length_max(air_limit);
        let world_drive = Vec3::new(0.0, 0.0, -9.81 * SECONDARY_GRAVITY)
            - accelerations[index] * 0.95
            + air_acceleration;
        instance
            .cloth
            .set_force(index as u32, solver_vector(inverse, world_drive) * mass)
            .map_err(|error| format!("cloth airflow force failed: {error}"))?;
        instance
            .cloth
            .set_velocity(
                index as u32,
                solver_vector(transport, old_velocities[index]),
            )
            .map_err(|error| format!("cloth velocity transport failed: {error}"))?;
    }
    instance.tick_targets.clone_from_slice(targets);
    instance.tick_motion = motion;
    instance.drive_velocities = Some(drive_velocities);
    instance.turn_velocities = Some(turn_velocities);
    Ok(())
}

fn interpolate_capsules(old: &[ClothCapsule], new: &[ClothCapsule], t: f32) -> Vec<ClothCapsule> {
    if old.len() != new.len() {
        return new.to_vec();
    }
    old.iter()
        .zip(new)
        .map(|(a, b)| ClothCapsule {
            a: std::array::from_fn(|i| a.a[i] + (b.a[i] - a.a[i]) * t),
            b: std::array::from_fn(|i| a.b[i] + (b.b[i] - a.b[i]) * t),
            radius: a.radius + (b.radius - a.radius) * t,
            basis: match (a.basis, b.basis) {
                (Some(a), Some(b)) => Some(interpolate_body_basis(a, b, t)),
                _ => None,
            },
        })
        .collect()
}

/// Clearance belongs to the posed reference surface. Asking secondary-offset
/// constraints to supply the entire gap can make their fixed rest lengths
/// incompatible with sewn attachments. Project the reference first, then let
/// native contacts handle physical displacement around that reference.
fn project_body_reference(
    targets: &mut [Vec3],
    authored: &[Vec3],
    capsules: &[ClothCapsule],
    pinned: &[u32],
    weights: &[f32],
    clearance: f32,
    particle_radius: f32,
) {
    let bodies = capsules.iter().map(BodyShape::new).collect::<Vec<_>>();
    for (i, target) in targets.iter_mut().enumerate() {
        if pinned.binary_search(&(i as u32)).is_ok() {
            continue;
        }
        let margin = clearance * weights[i] + particle_radius + 0.0005;
        let overlaps = bodies
            .iter()
            .map(|body| body.overlap(authored[i], particle_radius))
            .collect::<Vec<_>>();
        // Intersecting shapes form a body union. Follow its deepest boundary
        // instead of projecting a particle between opposing internal planes.
        for _ in 0..4 {
            let mut correction = Vec3::ZERO;
            let mut deepest = 0.0;
            for (body, overlap) in bodies.iter().zip(&overlaps) {
                if !body.may_contact(*target, margin) {
                    continue;
                }
                let (distance, _, normal) = body.surface(*target);
                let depth = margin - overlap - distance;
                if depth > deepest {
                    deepest = depth;
                    correction = normal * depth;
                }
            }
            if deepest <= 0.0001 {
                break;
            }
            *target += correction;
        }
    }
}

/// Bone-posed capsule, optionally stretched into an ellipsoid. Preparing the
/// inverse transform once avoids matrix work in every contact iteration.
pub(crate) struct BodyShape {
    a: Vec3,
    b: Vec3,
    bound_radius: f32,
    local_segment: Vec3,
    basis: glam::Mat3,
    inverse: glam::Mat3,
    min_radius: f32,
}
impl BodyShape {
    pub(crate) fn new(capsule: &ClothCapsule) -> Self {
        let basis = capsule
            .basis
            .map(|axes| glam::Mat3::from_cols_array_2d(&axes))
            .unwrap_or_else(|| {
                glam::Mat3::from_diagonal(glam::Vec3::splat(capsule.radius.max(0.01)))
            })
            * METRES_PER_JKA_UNIT;
        let inverse = basis.inverse();
        let a = to_metres(capsule.a);
        let local_segment = solver_vector(inverse, to_metres(capsule.b) - a);
        let min_radius = basis
            .to_cols_array_2d()
            .iter()
            .map(|axis| glam::Vec3::from_array(*axis).length())
            .fold(f32::INFINITY, f32::min)
            .max(0.0001);
        Self {
            a,
            b: to_metres(capsule.b),
            bound_radius: basis
                .to_cols_array_2d()
                .iter()
                .map(|axis| glam::Vec3::from_array(*axis).length_squared())
                .sum::<f32>()
                .sqrt(),
            local_segment,
            basis,
            inverse,
            min_radius,
        }
    }
    fn may_contact(&self, point: Vec3, margin: f32) -> bool {
        let distance = point.distance_squared(capsule_closest(point, self.a, self.b));
        distance <= (self.bound_radius + margin.max(0.0)).powi(2)
    }
    #[allow(dead_code)]
    pub(crate) fn penetration(&self, point: Vec3) -> f32 {
        if !self.may_contact(point, 0.0) {
            return 0.0;
        }
        (-self.surface(point).0).max(0.0)
    }
    fn overlap(&self, point: Vec3, radius: f32) -> f32 {
        if !self.may_contact(point, 0.0) {
            return 0.0;
        }
        (-self.surface(point).0 - radius).max(0.0)
    }
    /// Signed supporting-plane distance in metres, plus surface point/normal.
    pub(crate) fn surface(&self, point: Vec3) -> (f32, Vec3, Vec3) {
        let local = solver_vector(self.inverse, point - self.a);
        let center = capsule_closest(local, Vec3::ZERO, self.local_segment);
        let radial = (local - center).try_normalize().unwrap_or(Vec3::Z);
        let surface = self.a + solver_vector(self.basis, center + radial);
        let normal = solver_vector(self.inverse.transpose(), radial).normalize();
        ((point - surface).dot(normal), surface, normal)
    }
    fn swept_plane(&self, start: Vec3, end: Vec3, margin: f32) -> Option<(Vec3, Vec3)> {
        let (path, body) = closest_segments(start, end, self.a, self.b);
        if (path - body).length_squared() > (self.bound_radius + margin.max(0.0)).powi(2) {
            return None;
        }
        let start_local = solver_vector(self.inverse, start - self.a);
        let end_local = solver_vector(self.inverse, end - self.a);
        let (path, body) = closest_segments(start_local, end_local, Vec3::ZERO, self.local_segment);
        let bound = 1.0 + margin / self.min_radius;
        if (path - body).length_squared() > bound * bound || self.surface(start).0 <= margin {
            return None;
        }
        let (_, surface, normal) = self.surface(start);
        Some((surface, normal))
    }
}

fn interpolate_body_basis(a: [[f32; 3]; 3], b: [[f32; 3]; 3], t: f32) -> [[f32; 3]; 3] {
    // Direct matrix lerp collapses at a half-turn. Preserve positive scale
    // and interpolate the orthogonal bone rotation.
    let a = glam::Mat3::from_cols_array_2d(&a);
    let b = glam::Mat3::from_cols_array_2d(&b);
    let decompose = |matrix: glam::Mat3| {
        let scale = glam::Vec3::new(
            matrix.x_axis.length(),
            matrix.y_axis.length(),
            matrix.z_axis.length(),
        )
        .max(glam::Vec3::splat(0.001));
        let rotation =
            glam::Quat::from_mat3(&(matrix * glam::Mat3::from_diagonal(scale.recip()))).normalize();
        (scale, rotation)
    };
    let (a_scale, a_rotation) = decompose(a);
    let (b_scale, b_rotation) = decompose(b);
    (glam::Mat3::from_quat(a_rotation.slerp(b_rotation, t))
        * glam::Mat3::from_diagonal(a_scale.lerp(b_scale, t)))
    .to_cols_array_2d()
}

fn closest_segments(p: Vec3, q: Vec3, a: Vec3, b: Vec3) -> (Vec3, Vec3) {
    let d = q - p;
    let e = b - a;
    let r = p - a;
    let dd = d.length_squared();
    let ee = e.length_squared();
    let de = d.dot(e);
    let dr = d.dot(r);
    let er = e.dot(r);
    let (mut s, mut t);
    if dd <= 1.0e-10 {
        s = 0.0;
        t = if ee > 1.0e-10 {
            (er / ee).clamp(0.0, 1.0)
        } else {
            0.0
        };
    } else if ee <= 1.0e-10 {
        t = 0.0;
        s = (-dr / dd).clamp(0.0, 1.0);
    } else {
        let denominator = dd * ee - de * de;
        s = if denominator > 1.0e-10 {
            ((de * er - dr * ee) / denominator).clamp(0.0, 1.0)
        } else {
            0.0
        };
        t = (de * s + er) / ee;
        if t < 0.0 {
            t = 0.0;
            s = (-dr / dd).clamp(0.0, 1.0);
        } else if t > 1.0 {
            t = 1.0;
            s = ((de - dr) / dd).clamp(0.0, 1.0);
        }
    }
    (p + d * s, a + e * t)
}

fn fitted_opening_anchors(
    points: &[Vec3],
    triangles: &[[u32; 3]],
    capsules: &[ClothCapsule],
) -> Vec<u32> {
    let mut edges = HashMap::<(u32, u32), usize>::new();
    for &[a, b, c] in triangles {
        for (a, b) in [(a, b), (b, c), (c, a)] {
            *edges.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    let mut adjacent = HashMap::<u32, Vec<u32>>::new();
    for ((a, b), count) in edges {
        if count == 1 {
            adjacent.entry(a).or_default().push(b);
            adjacent.entry(b).or_default().push(a);
        }
    }
    let mut seeds = adjacent.keys().copied().collect::<Vec<_>>();
    seeds.sort_unstable();
    let mut seen = HashSet::new();
    let mut anchors = Vec::new();
    for seed in seeds {
        if !seen.insert(seed) {
            continue;
        }
        let mut queue = VecDeque::from([seed]);
        let mut ring = Vec::new();
        let mut closed = true;
        while let Some(v) = queue.pop_front() {
            ring.push(v);
            closed &= adjacent[&v].len() == 2;
            for &next in &adjacent[&v] {
                if seen.insert(next) {
                    queue.push_back(next);
                }
            }
        }
        if !closed || ring.len() < 3 {
            continue;
        }
        let center = ring.iter().map(|&i| points[i as usize]).sum::<Vec3>() / ring.len() as f32;
        let extent = ring
            .iter()
            .map(|&i| (points[i as usize] - center).length())
            .fold(0.0_f32, f32::max);
        let fitted = capsules.iter().any(|capsule| {
            let radius = capsule.radius * METRES_PER_JKA_UNIT;
            let nearest = capsule_closest(center, to_metres(capsule.a), to_metres(capsule.b));
            (center - nearest).length() <= radius * 0.75 + METRES_PER_JKA_UNIT
                && extent <= radius + 0.75 * METRES_PER_JKA_UNIT
        });
        if fitted {
            anchors.extend(ring);
        }
    }
    anchors.sort_unstable();
    anchors.dedup();
    anchors
}

fn capsule_closest(position: Vec3, a: Vec3, b: Vec3) -> Vec3 {
    let ab = b - a;
    let length_sq = ab.length_squared();
    let t = if length_sq > 1.0e-8 {
        ((position - a).dot(ab) / length_sq).clamp(0.0, 1.0)
    } else {
        0.0
    };
    a + ab * t
}

struct CapsuleContactSource<'a> {
    capsules: &'a [ClothCapsule],
    pinned: &'a [u32],
    reference: &'a [Vec3],
    targets: &'a [Vec3],
    fit_targets: &'a [Vec3],
    clearance_weights: &'a [f32],
    contact_limits: &'a [f32],
    prepared: Vec<BodyShape>,
    clearance: f32,
}

impl ContactSource for CapsuleContactSource<'_> {
    fn contacts(
        &mut self,
        previous: &[Vec3],
        positions: &[Vec3],
        radius: Real,
        stage: ContactStage,
        out: &mut Vec<Contact>,
    ) -> Result<(), ClothError> {
        const QUERY_MARGIN: f32 = 0.003;
        if self.prepared.len() != self.capsules.len() {
            self.prepared = self.capsules.iter().map(BodyShape::new).collect();
        }
        for (particle, &reference_position) in positions.iter().enumerate() {
            if self.pinned.binary_search(&(particle as u32)).is_ok() {
                continue;
            }
            let pose_delta = self.targets[particle] - self.reference[particle];
            let position = reference_position + pose_delta;
            let clearance =
                self.clearance * self.clearance_weights.get(particle).copied().unwrap_or(1.0);
            let mut best: Option<(f32, Contact)> = None;
            for (index, shape) in self.prepared.iter().enumerate() {
                let (distance, surface, normal) =
                    if shape.may_contact(position, clearance + radius + QUERY_MARGIN) {
                        shape.surface(position)
                    } else {
                        (f32::INFINITY, Vec3::ZERO, Vec3::ZERO)
                    };
                // Authored animation can place an arm inside the torso. A cloth
                // solver cannot repair that skeleton pose. Preserve only that
                // existing overlap; reject further dynamic penetration.
                let allowed_overlap = shape.overlap(self.fit_targets[particle], radius);
                let margin = clearance + radius - allowed_overlap;
                let (surface, normal) = if distance <= margin + QUERY_MARGIN {
                    (surface, normal)
                } else {
                    if stage == ContactStage::Stabilization {
                        continue;
                    }
                    let start = previous[particle] + pose_delta;
                    let Some(plane) = shape.swept_plane(start, position, margin.max(0.0)) else {
                        continue;
                    };
                    plane
                };
                let point = surface + normal * (clearance - allowed_overlap) - pose_delta;
                let depth = radius - (reference_position - point).dot(normal);
                if depth < -QUERY_MARGIN {
                    continue;
                }
                let contact = Contact {
                    key: ContactKey {
                        particle: particle as u32,
                        external: index as u64 + 1,
                        feature: 0,
                    },
                    normal,
                    point,
                    surface_velocity: Vec3::ZERO,
                    friction: 0.12,
                };
                // One boundary of the body union per particle avoids opposed
                // planes inside intersecting animated body parts.
                if best
                    .as_ref()
                    .is_none_or(|(old_depth, _)| depth > *old_depth)
                {
                    best = Some((depth, contact));
                }
            }
            if let Some((depth, mut contact)) = best {
                // Limit each native correction relative to local mesh spacing.
                // A single deep animated overlap must not collapse adjacent
                // triangles or inject an unbounded position correction.
                let limit = self
                    .contact_limits
                    .get(particle)
                    .copied()
                    .unwrap_or(f32::INFINITY);
                if depth > limit {
                    contact.point -= contact.normal * (depth - limit);
                }
                out.push(contact);
            }
        }
        Ok(())
    }
}

fn reset_reason(
    instance: &ClothInstance,
    targets: &[Vec3],
    motion: ClothMotion,
    now_ms: i32,
) -> Option<&'static str> {
    if now_ms < instance.last_time_ms {
        return Some("presentation clock moved backwards");
    }
    if now_ms.saturating_sub(instance.last_time_ms) > 250 {
        return Some("presentation gap >250ms");
    }
    if (to_metres(motion.origin) - to_metres(instance.frame_motion.origin)).length_squared()
        > TELEPORT_DISTANCE_METRES * TELEPORT_DISTANCE_METRES
        || targets
            .iter()
            .zip(&instance.frame_targets)
            .any(|(new, old)| {
                (*new - *old).length_squared() > TELEPORT_DISTANCE_METRES * TELEPORT_DISTANCE_METRES
            })
    {
        return Some("attachment teleport >1.5m");
    }
    None
}

fn maybe_print_debug(
    entity_num: u16,
    instance: &mut ClothInstance,
    targets: &[Vec3],
    report: Option<&StepReport>,
    frame_steps: u32,
    body_collision: bool,
    now_ms: i32,
) {
    if now_ms.saturating_sub(instance.last_debug_ms) < DEBUG_INTERVAL_MS {
        return;
    }
    instance.last_debug_ms = now_ms;

    let mut max_deviation = 0.0f32;
    let mut sum_deviation = 0.0f32;
    let mut samples = 0usize;
    for (position, target) in instance.cloth.positions().iter().zip(&instance.reference) {
        let deviation = (*position - *target).length();
        if deviation.is_finite() {
            max_deviation = max_deviation.max(deviation);
            sum_deviation += deviation;
            samples += 1;
        }
    }
    let average_deviation = if samples > 0 {
        sum_deviation / samples as f32
    } else {
        0.0
    };
    let max_speed = instance
        .cloth
        .velocities()
        .iter()
        .map(|velocity| velocity.length())
        .filter(|value| value.is_finite())
        .fold(0.0f32, f32::max);
    let mut animated_strains = Vec::with_capacity(instance.debug_edges.len());
    for &(a, b, rest_length) in &instance.debug_edges {
        let (Some(pa), Some(pb)) = (targets.get(a as usize), targets.get(b as usize)) else {
            continue;
        };
        let strain = (pa.distance(*pb) / rest_length - 1.0).abs();
        if strain.is_finite() {
            animated_strains.push(strain);
        }
    }
    animated_strains.sort_by(f32::total_cmp);
    let animated_max = animated_strains.last().copied().unwrap_or(0.0);
    let animated_p95 = if animated_strains.is_empty() {
        0.0
    } else {
        let index = ((animated_strains.len() as f32 * 0.95).ceil() as usize)
            .saturating_sub(1)
            .min(animated_strains.len() - 1);
        animated_strains[index]
    };

    let (
        max_stretch,
        p95_stretch,
        bend_error,
        target_error,
        penetration,
        contacts,
        stabilized_contacts,
        degenerate,
        iterations,
    ) = report
        .map(|report| {
            (
                report.max_stretch,
                report.p95_stretch,
                report.max_bend_error,
                report.max_target_error,
                report.max_penetration,
                report.contacts,
                report.stabilized_contacts,
                report.degenerate_faces,
                report.iterations,
            )
        })
        .unwrap_or((0.0, 0.0, 0.0, 0.0, 0.0, 0, 0, 0, 0));

    devprintln!(
        1,
        "CLOTHDBG ent={} model={} steps={} iters={} pins={} collision={} dev(max/avg)={:.2}/{:.2}u speed={:.1}ups offsetStretch(max/p95)={:.3}/{:.3} animStrain(max/p95)={:.3}/{:.3} bend={:.3} targetErr={:.2}u penetration={:.3}u contacts={}/stab{} degFaces={}",
        entity_num,
        instance.model_label,
        frame_steps,
        iterations,
        instance.pinned.len(),
        body_collision as u8,
        max_deviation / METRES_PER_JKA_UNIT,
        average_deviation / METRES_PER_JKA_UNIT,
        max_speed / METRES_PER_JKA_UNIT,
        max_stretch,
        p95_stretch,
        animated_max,
        animated_p95,
        bend_error,
        target_error / METRES_PER_JKA_UNIT,
        penetration / METRES_PER_JKA_UNIT,
        contacts,
        stabilized_contacts,
        degenerate,
    );
}

fn recompute_normals(
    positions: &[[f32; 3]],
    fallback: &[[f32; 3]],
    triangles: &[[u32; 3]],
) -> Vec<[f32; 3]> {
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for triangle in triangles {
        // GLM triangle winding is flipped when uploaded to the WGPU dynamic-model
        // path; match that visible front face here. Do not use the GPU index
        // array directly because two-sided materials duplicate every triangle
        // in reverse and would cancel their accumulated normals.
        let [a, b, c] = [
            triangle[0] as usize,
            triangle[2] as usize,
            triangle[1] as usize,
        ];
        let (Some(&pa), Some(&pb), Some(&pc)) =
            (positions.get(a), positions.get(b), positions.get(c))
        else {
            continue;
        };
        let pa = Vec3::from_array(pa);
        let pb = Vec3::from_array(pb);
        let pc = Vec3::from_array(pc);
        let face = (pb - pa).cross(pc - pa);
        if face.length_squared() > 1.0e-12 {
            normals[a] += face;
            normals[b] += face;
            normals[c] += face;
        }
    }
    normals
        .into_iter()
        .enumerate()
        .map(|(index, normal)| {
            if normal.length_squared() > 1.0e-12 {
                normal.normalize().to_array()
            } else {
                fallback.get(index).copied().unwrap_or([0.0, 1.0, 0.0])
            }
        })
        .collect()
}

#[inline]
fn to_metres(value: [f32; 3]) -> Vec3 {
    Vec3::new(value[0], value[1], value[2]) * METRES_PER_JKA_UNIT
}

#[inline]
fn from_metres(value: Vec3) -> [f32; 3] {
    (value / METRES_PER_JKA_UNIT).to_array()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn motion(x: f32, y: f32, z: f32, yaw: f32) -> ClothMotion {
        let (sin, cos) = yaw.sin_cos();
        ClothMotion {
            axis: [[cos, sin, 0.0], [-sin, cos, 0.0], [0.0, 0.0, 1.0]],
            origin: [x, y, z],
        }
    }

    fn robe() -> ClothSurfaceFrame {
        let mut positions = Vec::new();
        for row in 0..9 {
            for col in 0..7 {
                positions.push([-12.0 + col as f32 * 4.0, 10.0, 6.0 + row as f32 * 5.0]);
            }
        }
        let mut triangles = Vec::new();
        for row in 0..8 {
            for col in 0..6 {
                let a = (row * 7 + col) as u32;
                triangles.push([a, a + 1, a + 7]);
                triangles.push([a + 1, a + 8, a + 7]);
            }
        }
        ClothSurfaceFrame {
            surface_index: 0,
            surface_name: "hips_robe".to_owned(),
            bind_positions: positions.clone(),
            posed_positions: positions,
            posed_normals: vec![[0.0, -1.0, 0.0]; 63],
            skin_transforms: vec![
                [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0]
                ];
                63
            ],
            triangles,
        }
    }

    fn system(hz: u32, collision: bool) -> ClothSystem {
        let mut system = ClothSystem::default();
        system.set_config(ClothConfig {
            enabled: true,
            hz,
            body_collision: collision,
            animation_influence: 1.0,
            ..ClothConfig::default()
        });
        system
    }

    fn max_offset(output: &ClothOutput, surface: &ClothSurfaceFrame) -> f32 {
        output
            .positions
            .iter()
            .zip(&surface.posed_positions)
            .map(|(a, b)| Vec3::from_array(*a).distance(Vec3::from_array(*b)))
            .fold(0.0, f32::max)
    }

    #[test]
    fn overlapping_body_pins_do_not_abort_cloth() {
        let surface = robe();
        let root = motion(0.0, 0.0, 0.0, 0.0);
        // Deliberately too large: the old contact source reported pinned
        // particles inside this capsule and the native solver rejected it.
        let capsules = [ClothCapsule {
            a: [0.0, 0.0, 10.0],
            b: [0.0, 0.0, 48.0],
            radius: 20.0,
            basis: None,
        }];
        let mut system = system(60, true);
        for tick in 0..120 {
            let output = system
                .simulate_garment(
                    0,
                    "test",
                    0,
                    std::slice::from_ref(&surface),
                    &capsules,
                    root,
                    tick * 17,
                )
                .unwrap();
            assert!(max_offset(&output[&0], &surface) < 8.0);
        }
        let instance = system.instances.values().next().unwrap();
        let mut source = CapsuleContactSource {
            capsules: &capsules,
            pinned: &instance.pinned,
            reference: &instance.reference,
            targets: &instance.frame_targets,
            fit_targets: &instance.frame_authored_targets,
            clearance_weights: &instance.clearance_weights,
            contact_limits: &instance.contact_limits,
            prepared: Vec::new(),
            clearance: 0.0,
        };
        let mut contacts = Vec::new();
        source
            .contacts(
                instance.cloth.positions(),
                instance.cloth.positions(),
                instance.cloth.material().contact_radius,
                ContactStage::Final,
                &mut contacts,
            )
            .unwrap();
        assert!(contacts
            .iter()
            .all(|c| instance.pinned.binary_search(&c.key.particle).is_err()));
    }

    #[test]
    fn authored_edge_strain_preserves_shape_without_solver_conflict() {
        let mut surface = robe();
        let mut system = system(60, false);
        for tick in 0..240 {
            let stretch = 1.0 + 0.45 * (tick as f32 * 0.025).sin();
            for (posed, bind) in surface
                .posed_positions
                .iter_mut()
                .zip(&surface.bind_positions)
            {
                *posed = [bind[0] * stretch, bind[1], bind[2] * stretch];
            }
            let output = system
                .simulate_garment(
                    0,
                    "test",
                    0,
                    std::slice::from_ref(&surface),
                    &[],
                    motion(0.0, 0.0, 0.0, 0.0),
                    tick * 17,
                )
                .unwrap();
            assert!(
                max_offset(&output[&0], &surface) < 10.0,
                "authored strain caused runaway"
            );
            let instance = system.instances.values().next().unwrap();
            for &pin in &instance.pinned {
                assert_eq!(
                    instance.cloth.positions()[pin as usize],
                    instance.reference[pin as usize]
                );
            }
        }
    }

    #[test]
    fn starts_stops_and_turns_drive_visible_secondary_motion() {
        let surface = robe();
        let mut idle = system(60, false);
        let mut moving = system(60, false);
        let mut max_difference = 0.0f32;
        for tick in 0..180 {
            let t = tick as f32 / 60.0;
            let distance = if t < 0.5 {
                0.0
            } else if t < 1.5 {
                (t - 0.5) * 180.0
            } else {
                180.0
            };
            let yaw = if t < 1.5 {
                0.0
            } else {
                ((t - 1.5) * 3.0).min(2.5)
            };
            let a = idle
                .simulate_garment(
                    0,
                    "test",
                    0,
                    std::slice::from_ref(&surface),
                    &[],
                    motion(0.0, 0.0, 0.0, 0.0),
                    tick * 17,
                )
                .unwrap();
            let b = moving
                .simulate_garment(
                    0,
                    "test",
                    0,
                    std::slice::from_ref(&surface),
                    &[],
                    motion(distance, 0.0, 0.0, yaw),
                    tick * 17,
                )
                .unwrap();
            let difference = a[&0]
                .positions
                .iter()
                .zip(&b[&0].positions)
                .map(|(a, b)| Vec3::from_array(*a).distance(Vec3::from_array(*b)))
                .fold(0.0, f32::max);
            max_difference = max_difference.max(difference);
            assert!(max_offset(&b[&0], &surface) < 12.0);
        }
        assert!(
            max_difference > 0.5,
            "movement failed to produce visible cloth lag: {max_difference}"
        );
    }

    #[test]
    fn repeated_jumps_and_low_tick_rate_remain_bounded() {
        let surface = robe();
        let capsules = [ClothCapsule {
            a: [0.0, 0.0, 10.0],
            b: [0.0, 0.0, 48.0],
            radius: 9.0,
            basis: None,
        }];
        for hz in [15, 60, 240] {
            let mut system = system(hz, true);
            for tick in 0..300 {
                let t = (tick as f32 * 0.017) % 1.0;
                let z = if t < 0.7 {
                    (240.0 * t - 340.0 * t * t).max(0.0)
                } else {
                    0.0
                };
                let output = system
                    .simulate_garment(
                        0,
                        "test",
                        0,
                        std::slice::from_ref(&surface),
                        &capsules,
                        motion(0.0, 0.0, z, 0.0),
                        tick * 17,
                    )
                    .unwrap();
                assert!(
                    max_offset(&output[&0], &surface) < 12.0,
                    "jump runaway at {hz}Hz, tick={tick}, offset={}",
                    max_offset(&output[&0], &surface)
                );
                assert!(output[&0].positions.iter().flatten().all(|v| v.is_finite()));
            }
        }
    }

    #[test]
    fn high_render_rate_does_not_change_motion_response() {
        fn run(frame_ms: i32) -> Vec<[f32; 3]> {
            let surface = robe();
            let mut system = system(60, false);
            let mut final_positions = Vec::new();
            for now in (0..=2400).step_by(frame_ms as usize) {
                let t = now as f32 * 0.001;
                let x = 60.0 * (1.0 - (t * 2.0).cos());
                let output = system
                    .simulate_garment(
                        0,
                        "test",
                        0,
                        std::slice::from_ref(&surface),
                        &[],
                        motion(x, 0.0, 0.0, t * 0.8),
                        now,
                    )
                    .unwrap();
                final_positions = output[&0].positions.clone();
            }
            final_positions
        }
        let slow = run(16);
        let fast = run(2);
        let error = slow
            .iter()
            .zip(fast)
            .map(|(a, b)| Vec3::from_array(*a).distance(Vec3::from_array(b)))
            .fold(0.0, f32::max);
        assert!(
            error < 0.25,
            "render rate changed secondary motion by {error} units"
        );
    }

    #[test]
    fn head_contacts_use_posed_geometry_in_reference_coordinates() {
        let reference = [Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0) * METRES_PER_JKA_UNIT];
        let targets = [
            Vec3::new(20.0, 0.0, 0.0) * METRES_PER_JKA_UNIT,
            Vec3::new(30.0, 0.0, 0.0) * METRES_PER_JKA_UNIT,
        ];
        let positions = [Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0) * METRES_PER_JKA_UNIT];
        let capsules = [ClothCapsule {
            a: [20.0, 0.0, 0.0],
            b: [20.0, 0.0, 0.0],
            radius: 5.5,
            basis: None,
        }];
        let mut source = CapsuleContactSource {
            capsules: &capsules,
            pinned: &[0],
            reference: &reference,
            targets: &targets,
            fit_targets: &targets,
            clearance_weights: &[],
            contact_limits: &[],
            prepared: Vec::new(),
            clearance: 0.0,
        };
        let mut contacts = Vec::new();
        source
            .contacts(
                &positions,
                &positions,
                0.005,
                ContactStage::Iteration,
                &mut contacts,
            )
            .unwrap();
        assert_eq!(contacts.len(), 1);
        let contact = contacts[0];
        assert_eq!(contact.key.particle, 1);
        assert!(contact.normal.dot(Vec3::X) > 0.99);
        assert!((positions[1] - contact.point).dot(contact.normal) < 0.005);
        assert!((reference[1] - contact.point).dot(contact.normal) > 0.005);
        assert!(
            contact.point.x < 0.2,
            "contact plane was left in posed coordinates"
        );
    }

    #[test]
    fn half_turn_interpolation_and_teleports_do_not_inject_runaway_motion() {
        let root = motion(0.0, 0.0, 0.0, 0.0);
        let turn = motion(0.0, 0.0, 0.0, std::f32::consts::PI);
        assert!((root.interpolate(turn, 0.5).matrix().determinant() - 1.0).abs() < 1.0e-4);
        let surface = robe();
        let mut system = system(60, false);
        system
            .simulate_garment(0, "test", 0, std::slice::from_ref(&surface), &[], root, 0)
            .unwrap();
        system
            .simulate_garment(0, "test", 0, std::slice::from_ref(&surface), &[], turn, 34)
            .unwrap();
        let teleport = motion(1000.0, -500.0, 200.0, std::f32::consts::PI);
        let output = system
            .simulate_garment(
                0,
                "test",
                0,
                std::slice::from_ref(&surface),
                &[],
                teleport,
                51,
            )
            .unwrap();
        assert!(max_offset(&output[&0], &surface) < 1.0);
    }

    #[test]
    fn unused_render_vertices_keep_their_authored_pose() {
        let mut surface = robe();
        let unused = [100.0, -100.0, 80.0];
        surface.bind_positions.push(unused);
        surface.posed_positions.push(unused);
        surface.posed_normals.push([0.0, -1.0, 0.0]);
        surface.skin_transforms.push(surface.skin_transforms[0]);
        let mut system = system(60, false);
        let output = system
            .simulate_garment(
                0,
                "test",
                0,
                std::slice::from_ref(&surface),
                &[],
                motion(0.0, 0.0, 0.0, 0.0),
                0,
            )
            .unwrap();
        assert_eq!(output[&0].positions.last(), Some(&unused));
    }

    #[test]
    fn welded_render_seams_share_positions_and_smooth_normals() {
        let identity = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ];
        let make = |index, name: &str, bind: Vec<[f32; 3]>| ClothSurfaceFrame {
            surface_index: index,
            surface_name: name.to_owned(),
            bind_positions: bind.clone(),
            posed_positions: bind,
            posed_normals: vec![[0.0, -1.0, 0.0]; 4],
            skin_transforms: vec![identity; 4],
            triangles: vec![[0, 1, 2], [1, 3, 2]],
        };
        let upper = make(
            0,
            "torso_robe",
            vec![
                [-5.0, 10.0, 10.0],
                [5.0, 10.0, 10.0],
                [-5.0, 10.0, 20.0],
                [5.0, 10.0, 20.0],
            ],
        );
        let mut lower = make(
            1,
            "hips_robe",
            vec![
                [-5.0, 11.0, 0.0],
                [5.0, 11.0, 0.0],
                [-5.0, 10.0, 10.4],
                [5.0, 10.0, 10.4],
            ],
        );
        // Both edges match within the 1-unit bind tolerance, but their authored
        // skinning now pulls them apart. Rendering must still share the seam.
        lower.posed_positions[2][1] += 2.0;
        lower.posed_positions[3][1] += 2.0;
        lower.skin_transforms[2][1][3] = 2.0;
        lower.skin_transforms[3][1][3] = 2.0;
        let mut system = system(60, false);
        let frames = [upper, lower];
        let output = system
            .simulate_garment(0, "seam", 0, &frames, &[], motion(0.0, 0.0, 0.0, 0.0), 0)
            .unwrap();
        for (a, b) in [(0, 2), (1, 3)] {
            assert_eq!(output[&0].positions[a], output[&1].positions[b]);
            assert_eq!(output[&0].normals[a], output[&1].normals[b]);
        }
        // A deliberate authored crease retains separate lighting normals.
        let mut frames = frames;
        frames[1].posed_normals.fill([0.0, 0.0, 1.0]);
        frames[1].posed_positions[0][1] = 20.0;
        frames[1].posed_positions[1][1] = 20.0;
        let output = system
            .simulate_garment(0, "seam", 0, &frames, &[], motion(0.0, 0.0, 0.0, 0.0), 1)
            .unwrap();
        assert!(
            Vec3::from_array(output[&0].normals[0]).dot(Vec3::from_array(output[&1].normals[2]))
                < 0.99
        );
    }

    #[test]
    fn free_fabric_uses_attachment_motion_without_bone_or_animation_rules() {
        let mut surface = robe();
        let root = motion(0.0, 0.0, 0.0, 0.0);
        let mut system = system(60, false);
        system
            .simulate_garment(
                0,
                "generic",
                0,
                std::slice::from_ref(&surface),
                &[],
                root,
                0,
            )
            .unwrap();
        let instance = system.instances.values().next().unwrap();
        assert!(instance.animation_freedom[0] > 0.99);
        // Arbitrary skeleton deformation moves the free bottom. Attachment
        // transforms remain fixed. Reduced influence keeps hanging shape.
        surface.posed_positions[0][2] += 20.0;
        surface.skin_transforms[0][2][3] += 20.0;
        let flat = flatten_positions(std::slice::from_ref(&surface), |s| &s.posed_positions);
        let authored = particle_targets(&instance.particle_members, &flat);
        let free = garment_targets(instance, std::slice::from_ref(&surface), &authored, 0.0);
        let mixed = garment_targets(instance, std::slice::from_ref(&surface), &authored, 0.35);
        let full = garment_targets(instance, std::slice::from_ref(&surface), &authored, 1.0);
        assert!((from_metres(free[0])[2] - 6.0).abs() < 0.001);
        assert!((from_metres(mixed[0])[2] - 13.0).abs() < 0.001);
        assert!((from_metres(full[0])[2] - 26.0).abs() < 0.001);
        for (position, transform) in surface
            .posed_positions
            .iter_mut()
            .zip(&mut surface.skin_transforms)
        {
            position[0] += 15.0;
            transform[0][3] += 15.0;
        }
        let flat = flatten_positions(std::slice::from_ref(&surface), |s| &s.posed_positions);
        let authored = particle_targets(&instance.particle_members, &flat);
        let moved = garment_targets(instance, std::slice::from_ref(&surface), &authored, 0.0);
        assert!(
            (from_metres(moved[0])[0] - 3.0).abs() < 0.001,
            "free fabric failed to follow moving attachments"
        );
        for &pin in &instance.pinned {
            assert_eq!(moved[pin as usize], authored[pin as usize]);
        }
    }

    #[test]
    fn airflow_uses_relative_speed_face_orientation_and_quadratic_pressure() {
        let surface = robe();
        let root = motion(0.0, 0.0, 0.0, 0.0);
        let mut system = system(60, false);
        system
            .simulate_garment(0, "air", 0, std::slice::from_ref(&surface), &[], root, 0)
            .unwrap();
        let instance = system.instances.values().next().unwrap();
        let velocities = vec![Vec3::ZERO; instance.reference.len()];
        let total = |forces: Vec<Vec3>| forces.into_iter().sum::<Vec3>();
        let normal = total(aerodynamic_forces(
            instance,
            &instance.reference,
            root,
            &velocities,
            Vec3::new(0.0, -5.0, 0.0),
            1.0,
        ));
        let faster = total(aerodynamic_forces(
            instance,
            &instance.reference,
            root,
            &velocities,
            Vec3::new(0.0, -10.0, 0.0),
            1.0,
        ));
        let edge = total(aerodynamic_forces(
            instance,
            &instance.reference,
            root,
            &velocities,
            Vec3::new(5.0, 0.0, 0.0),
            1.0,
        ));
        assert!(normal.y < 0.0);
        assert!((faster.y / normal.y - 4.0).abs() < 0.001);
        assert!(normal.length() > edge.length() * 5.0);
        let matching = vec![Vec3::new(0.0, -5.0, 0.0); velocities.len()];
        let still_air = total(aerodynamic_forces(
            instance,
            &instance.reference,
            root,
            &matching,
            Vec3::new(0.0, -5.0, 0.0),
            1.0,
        ));
        assert_eq!(still_air, Vec3::ZERO);
        let movement = total(aerodynamic_forces(
            instance,
            &instance.reference,
            root,
            &matching,
            Vec3::ZERO,
            1.0,
        ));
        assert!((movement + normal).length() < 0.001);
        assert_eq!(
            total(aerodynamic_forces(
                instance,
                &instance.reference,
                root,
                &matching,
                Vec3::ZERO,
                0.0
            )),
            Vec3::ZERO
        );
    }

    #[test]
    fn wind_and_default_attachment_influence_remain_stable_during_jumps() {
        let surface = robe();
        let mut system = system(60, true);
        let mut config = system.config;
        config.animation_influence = 0.35;
        config.wind_velocity = [200.0, -120.0, 0.0];
        system.set_config(config);
        let capsules = [ClothCapsule {
            a: [0.0, 0.0, 10.0],
            b: [0.0, 0.0, 48.0],
            radius: 9.0,
            basis: None,
        }];
        for tick in 0..240 {
            let phase = (tick as f32 * 0.017) % 1.0;
            let jump = (240.0 * phase - 340.0 * phase * phase).max(0.0);
            let output = system
                .simulate_garment(
                    0,
                    "wind",
                    0,
                    std::slice::from_ref(&surface),
                    &capsules,
                    motion(tick as f32 * 2.0, 0.0, jump, phase),
                    tick * 17,
                )
                .unwrap();
            assert!(max_offset(&output[&0], &surface) < 15.0);
            assert!(output[&0].positions.iter().flatten().all(|v| v.is_finite()));
        }
    }
}

#[cfg(test)]
mod body_contact_regressions {
    use super::*;
    fn capsule(center: [f32; 3], radius: f32) -> ClothCapsule {
        ClothCapsule {
            a: center,
            b: center,
            radius,
            basis: None,
        }
    }
    #[test]
    fn stretched_body_surface_preserves_thin_front_back_extent() {
        let capsule = ClothCapsule {
            a: [0.0; 3],
            b: [0.0; 3],
            radius: 7.0,
            basis: Some([[7.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 0.0, 3.0]]),
        };
        let shape = BodyShape::new(&capsule);
        assert!(shape.surface(to_metres([0.0, 2.0, 0.0])).0.abs() < 1e-6);
        assert!(shape.surface(to_metres([7.0, 0.0, 0.0])).0.abs() < 1e-6);
        assert!(shape.surface(to_metres([0.0, 3.0, 0.0])).0 > 0.02);
    }
    #[test]
    fn body_shape_half_turn_keeps_a_finite_nonzero_volume() {
        let a = glam::Mat3::from_diagonal(glam::Vec3::new(7.0, 2.0, 3.0));
        let b = glam::Mat3::from_rotation_z(std::f32::consts::PI) * a;
        let basis = interpolate_body_basis(a.to_cols_array_2d(), b.to_cols_array_2d(), 0.5);
        let matrix = glam::Mat3::from_cols_array_2d(&basis);
        assert!(matrix.determinant() > 40.0);
        let shape = BodyShape::new(&ClothCapsule {
            basis: Some(basis),
            ..capsule([0.0; 3], 7.0)
        });
        let (distance, point, normal) = shape.surface(to_metres([0.0, 8.0, 0.0]));
        assert!(distance.is_finite() && point.is_finite() && normal.is_finite());
    }
    #[test]
    fn sweeps_share_the_current_pose_frame_and_skip_stabilization() {
        let capsules = [capsule([20.0, 0.0, 0.0], 5.5)];
        let reference = [to_metres([10.0, 0.0, 0.0])];
        let targets = [to_metres([30.0, 0.0, 0.0])];
        let previous = [to_metres([-10.0, 0.0, 0.0])];
        let positions = [to_metres([10.0, 0.0, 0.0])];
        let mut source = CapsuleContactSource {
            capsules: &capsules,
            pinned: &[],
            reference: &reference,
            targets: &targets,
            fit_targets: &targets,
            clearance_weights: &[],
            contact_limits: &[],
            prepared: Vec::new(),
            clearance: 0.0,
        };
        let mut contacts = Vec::new();
        source
            .contacts(
                &previous,
                &positions,
                0.005,
                ContactStage::Stabilization,
                &mut contacts,
            )
            .unwrap();
        assert!(contacts.is_empty());
        source
            .contacts(
                &previous,
                &positions,
                0.005,
                ContactStage::Prediction,
                &mut contacts,
            )
            .unwrap();
        assert_eq!(contacts.len(), 1);
        assert!(contacts[0].normal.dot(-Vec3::X) > 0.99);
        assert!((contacts[0].point.x - to_metres([-5.5, 0.0, 0.0]).x).abs() < 0.001);
    }
    #[test]
    fn overlapping_body_parts_have_one_contact_boundary_per_particle() {
        let capsules = [capsule([0.0; 3], 2.0), capsule([3.0, 0.0, 0.0], 2.0)];
        let targets = [to_metres([1.5, 0.0, 0.0])];
        let authored = [to_metres([1.5, 1.0, 0.0])];
        let mut source = CapsuleContactSource {
            capsules: &capsules,
            pinned: &[],
            reference: &targets,
            targets: &targets,
            fit_targets: &authored,
            clearance_weights: &[],
            contact_limits: &[],
            prepared: Vec::new(),
            clearance: 0.0254,
        };
        let mut contacts = Vec::new();
        source
            .contacts(
                &targets,
                &targets,
                0.005,
                ContactStage::Iteration,
                &mut contacts,
            )
            .unwrap();
        assert_eq!(contacts.len(), 1);
        assert!(contacts[0].normal.is_finite());
    }
    #[test]
    fn clearance_moves_the_supported_reference_without_detaching_pins() {
        let authored = [to_metres([7.0, 0.0, 0.0]), to_metres([7.0, 0.0, 0.0])];
        let mut targets = authored;
        let capsules = [capsule([0.0; 3], 5.0)];
        project_body_reference(
            &mut targets,
            &authored,
            &capsules,
            &[0],
            &[0.0, 1.0],
            4.0 * METRES_PER_JKA_UNIT,
            0.005,
        );
        assert_eq!(targets[0], authored[0]);
        assert!(targets[1].x > to_metres([9.0, 0.0, 0.0]).x);
        let mut source = CapsuleContactSource {
            capsules: &capsules,
            pinned: &[0],
            reference: &authored,
            targets: &targets,
            fit_targets: &authored,
            clearance_weights: &[0.0, 1.0],
            contact_limits: &[],
            prepared: Vec::new(),
            clearance: 4.0 * METRES_PER_JKA_UNIT,
        };
        let mut contacts = Vec::new();
        source
            .contacts(
                &authored,
                &authored,
                0.005,
                ContactStage::Stabilization,
                &mut contacts,
            )
            .unwrap();
        assert!(contacts
            .iter()
            .all(|c| (authored[c.key.particle as usize] - c.point).dot(c.normal) >= 0.005));
    }
}

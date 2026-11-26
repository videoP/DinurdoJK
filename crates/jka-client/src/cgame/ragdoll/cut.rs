//! Dynamic-cut split of a live Rapier corpse into independent sub-ragdolls.
//!
//! The presenter decides which simulated segments end up in which piece (a
//! skeleton-graph cut, see `player_presenter::bodies::dynamic_cut`). Here the
//! existing rigid bodies are moved between instances in place: velocities and
//! contacts carry through, joints that span two pieces are removed, and a
//! crossed segment keeps only the collider length on its joint side.

use super::*;

/// Instance keys at or above this value are cut pieces, not entities
/// (entity numbers are below MAX_GENTITIES = 1024).
pub(crate) const CUT_PIECE_KEY_BASE: u16 = 0x8000;
const CUT_PIECE_KEY_LAST: u16 = 0xfffe;
/// Approximate tissue density for a synthesized piece, kg per Rapier unit^3.
const TISSUE_DENSITY: f32 = 985.0;

#[derive(Debug, Clone)]
pub(super) struct PieceState {
    /// Unscaled entity matrix of the source corpse at the split. A piece no
    /// longer follows the server corpse origin.
    pub frame: Matrix3x4,
    /// Driven bone whose delta carries every bone this piece doesn't simulate,
    /// so vertices weighted across the cut move rigidly with the piece.
    pub anchor_bone: usize,
    /// Per GLA bone: the driven bone whose delta it follows (None = anchor).
    /// The piece's driven set never changes, so this is resolved once.
    pub drivers: Vec<Option<usize>>,
}

pub(crate) struct RagdollCutGroup {
    pub key: u16,
    /// Driven GLA bones moving into this piece. The first is the anchor.
    pub bones: Vec<usize>,
    /// A piece that owns no simulated segment gets one new rigid body that
    /// starts exactly at this driven bone's body pose, sized to these points
    /// given in that body's local frame (JKA units, modelScale applied).
    pub synthesize: Option<(usize, Vec<[f32; 3]>)>,
    /// Extra linear velocity in JKA units/s (saber follow-through).
    pub kick: [f32; 3],
}

pub(crate) struct RagdollCut {
    pub source: u16,
    /// Simulated segments the plane crossed: (driven bone, fraction along the
    /// segment from its joint). The body keeps its joint end, so its capsule
    /// is shortened to the cut. Pose independent, so a cut computed on a
    /// worker can be applied a frame or two later.
    pub truncate: Vec<(usize, f32)>,
    pub groups: Vec<RagdollCutGroup>,
}

impl RagdollWorld {
    pub(crate) fn instance_serial(&self, key: u16) -> Option<u64> {
        self.instances.get(&key).map(|instance| instance.serial)
    }

    /// GLA bone indices of the simulated segments currently owned by `key`.
    pub(crate) fn driven_bone_indices(&self, key: u16) -> Option<Vec<usize>> {
        self.instances
            .get(&key)
            .map(|instance| instance.driven.iter().map(|driven| driven.bone_index).collect())
    }

    pub(crate) fn alloc_cut_piece_key(&mut self) -> Option<u16> {
        let span = usize::from(CUT_PIECE_KEY_LAST - CUT_PIECE_KEY_BASE) + 1;
        for _ in 0..span {
            let key = self.next_piece_key;
            self.next_piece_key = if key >= CUT_PIECE_KEY_LAST { CUT_PIECE_KEY_BASE } else { key + 1 };
            if !self.instances.contains_key(&key) {
                return Some(key);
            }
        }
        None
    }

    /// Detached-limb budget/lifetime (`r_dismemberMax`, `r_dismemberLifetime`).
    pub(super) fn expire_cut_pieces(&mut self, now: i32, lifetime_ms: i32) {
        let mut pieces: Vec<(u16, i32)> = self
            .instances
            .iter()
            .filter(|(_, instance)| instance.piece.is_some())
            .map(|(&key, instance)| (key, instance.spawned_at))
            .collect();
        pieces.sort_unstable_by_key(|&(key, spawned)| (spawned, if self.fixed_reference { key } else { 0 }));
        let excess = pieces.len().saturating_sub(self.config.max_detached_limbs as usize);
        for (index, &(key, spawned)) in pieces.iter().enumerate() {
            if index < excess || (lifetime_ms > 0 && now.saturating_sub(spawned) >= lifetime_ms) {
                self.remove_instance(key);
            }
        }
    }

    /// Solved pose of a cut piece plus the frame/scale it renders in.
    pub(crate) fn cut_piece_pose(&self, key: u16) -> Option<(Vec<Matrix3x4>, Matrix3x4, f32)> {
        let instance = self.instances.get(&key)?;
        let piece = instance.piece.as_ref()?;
        let entity_inverse = inverse_affine(&piece.frame)?;
        let deltas = self.driven_deltas(&instance.driven, &entity_inverse, instance.model_scale);
        let anchor = deltas
            .get(&piece.anchor_bone)
            .or_else(|| deltas.values().next())
            .copied()?;
        let pose = instance
            .spawn_pose
            .iter()
            .enumerate()
            .map(|(index, spawn)| {
                let delta = piece
                    .drivers
                    .get(index)
                    .copied()
                    .flatten()
                    .and_then(|driver| deltas.get(&driver))
                    .unwrap_or(&anchor);
                multiply_3x4(delta, spawn)
            })
            .collect();
        Some((pose, piece.frame, instance.model_scale))
    }

    /// Move the listed segments out of `cut.source` into new piece instances.
    /// Returns (key, serial) per created piece; empty when nothing changed.
    pub(crate) fn split_for_cut(&mut self, cut: RagdollCut, gla: &GlaAnimation, now: i32) -> Vec<(u16, u64)> {
        let Some(mut source) = self.instances.remove(&cut.source) else {
            return Vec::new();
        };
        let body_of = |driven: &[DrivenBone], bone: usize| {
            driven.iter().find(|d| d.bone_index == bone).copied()
        };

        // Shorten crossed segments before their bodies are redistributed.
        for &(bone, fraction) in &cut.truncate {
            if let Some(driven) = body_of(&source.driven, bone) {
                self.truncate_collider(driven.body, fraction);
            }
        }

        // Group 0 is the source; every listed piece takes its bodies.
        let mut group_of = HashMap::<RigidBodyHandle, usize>::new();
        for driven in &source.driven {
            group_of.insert(driven.body, 0);
        }
        let mut piece_driven: Vec<Vec<DrivenBone>> = Vec::with_capacity(cut.groups.len());
        for (index, group) in cut.groups.iter().enumerate() {
            let mut moved = Vec::new();
            for &bone in &group.bones {
                if let Some(position) = source.driven.iter().position(|d| d.bone_index == bone) {
                    let driven = source.driven.remove(position);
                    group_of.insert(driven.body, index + 1);
                    moved.push(driven);
                }
            }
            piece_driven.push(moved);
        }

        // Synthesized bodies copy the crossed segment's current motion, so
        // the first delta equals the visible pose and nothing pops.
        for (index, group) in cut.groups.iter().enumerate() {
            let Some((copy_bone, points)) = &group.synthesize else { continue };
            let template = body_of(&source.driven, *copy_bone)
                .or_else(|| piece_driven.iter().flatten().find(|d| d.bone_index == *copy_bone).copied());
            let Some(template) = template else { continue };
            if let Some(driven) = self.synthesize_piece_body(template, points) {
                group_of.insert(driven.body, index + 1);
                piece_driven[index].push(driven);
            }
        }

        // Joints spanning two groups no longer hold anything together.
        let mut severed = HashSet::new();
        for &body in group_of.keys() {
            for (a, b, handle, _) in self.impulse_joints.attached_joints(body) {
                let (ga, gb) = (group_of.get(&a), group_of.get(&b));
                if ga.is_some() && gb.is_some() && ga != gb {
                    severed.insert(handle);
                }
            }
        }
        if self.fixed_reference {
            let mut handles: Vec<_> = severed.into_iter().collect();
            handles.sort_unstable_by_key(|h| h.into_raw_parts());
            for handle in handles { self.impulse_joints.remove(handle, true); }
        } else {
            for handle in severed { self.impulse_joints.remove(handle, true); }
        }

        let mut soft_by_group: Vec<Vec<SoftJoint>> =
            (0..=cut.groups.len()).map(|_| Vec::new()).collect();
        for joint in source.soft_joints.drain(..) {
            match (group_of.get(&joint.parent), group_of.get(&joint.child)) {
                (Some(&a), Some(&b)) if a == b => soft_by_group[a].push(joint),
                _ => {}
            }
        }
        let mut smear_by_group: Vec<Vec<CorpseSmearPoint>> =
            (0..=cut.groups.len()).map(|_| Vec::new()).collect();
        for point in source.corpse_smear_points.drain(..) {
            if let Some(&group) = group_of.get(&point.body) {
                smear_by_group[group].push(point);
            }
        }
        source.soft_joints = std::mem::take(&mut soft_by_group[0]);
        source.corpse_smear_points = std::mem::take(&mut smear_by_group[0]);

        let mut created = Vec::new();
        for (index, (group, driven)) in cut.groups.iter().zip(piece_driven).enumerate() {
            if driven.is_empty() {
                continue;
            }
            let kick = Vector::new(group.kick[0], group.kick[1], group.kick[2]) * JKA_TO_RAPIER;
            for d in &driven {
                if let Some(body) = self.bodies.get_mut(d.body) {
                    let velocity = body.linvel() + kick;
                    body.set_linvel(velocity, true);
                }
            }
            let anchor_bone = group
                .bones
                .iter()
                .copied()
                .find(|bone| driven.iter().any(|d| d.bone_index == *bone))
                .unwrap_or(driven[0].bone_index);
            self.next_serial = self.next_serial.wrapping_add(1);
            let serial = self.next_serial;
            let drivers = (0..gla.skeleton.len())
                .map(|bone| {
                    ragdoll_driver_for_bone_by(gla, bone, |b| driven.iter().any(|d| d.bone_index == b))
                })
                .collect();
            self.instances.insert(group.key, RagdollInstance {
                model_key: source.model_key.clone(),
                model_scale: source.model_scale,
                generation: source.generation,
                kind: RagdollKind::Corpse,
                grip_anchor: None,
                torso_anchor: None,
                spawned_at: now,
                lifetime_slept: false,
                last_authoritative_origin: source.last_authoritative_origin,
                spawn_pose: source.spawn_pose.clone(),
                last_applied_pose: source.last_applied_pose.clone(),
                driven,
                soft_joints: std::mem::take(&mut soft_by_group[index + 1]),
                corpse_smear_points: std::mem::take(&mut smear_by_group[index + 1]),
                next_corpse_smear_ms: now,
                serial,
                last_frame: source.last_frame,
                piece: Some(PieceState { frame: source.last_frame, anchor_bone, drivers }),
            });
            created.push((group.key, serial));
        }
        self.instances.insert(cut.source, source);
        created
    }

    fn truncate_collider(&mut self, body_handle: RigidBodyHandle, fraction: f32) {
        let Some(body) = self.bodies.get(body_handle) else { return };
        let handles = body.colliders().to_vec();
        for handle in handles {
            let Some(collider) = self.colliders.get(handle) else { continue };
            let Some(capsule) = collider.shape().as_capsule() else { continue };
            let axis = capsule.segment.b - capsule.segment.a;
            let local_cut = capsule.segment.a + axis * fraction.clamp(0.0, 1.0);
            let full = axis.length();
            let kept = (local_cut - capsule.segment.a).length();
            if full <= 1.0e-4 || kept >= full {
                continue;
            }
            let replacement = if kept > capsule.radius * 0.5 {
                ColliderBuilder::capsule_from_endpoints(Vector::ZERO, local_cut, capsule.radius)
            } else {
                ColliderBuilder::ball(capsule.radius)
            }
            .mass((collider.mass() * (kept / full).clamp(0.25, 1.0)).max(0.05))
            .friction(collider.friction())
            .restitution(0.0)
            .collision_groups(collider.collision_groups());
            self.colliders.remove(handle, &mut self.islands, &mut self.bodies, true);
            self.colliders.insert_with_parent(replacement, body_handle, &mut self.bodies);
        }
    }

    fn synthesize_piece_body(&mut self, template: DrivenBone, points: &[[f32; 3]]) -> Option<DrivenBone> {
        let source = self.bodies.get(template.body)?;
        let pose = source.position().clone();
        let (linvel, angvel) = (source.linvel(), source.angvel());
        let groups = source
            .colliders()
            .first()
            .and_then(|handle| self.colliders.get(*handle))
            .map(|collider| collider.collision_groups())?;
        let mut lo = Vector::splat(f32::INFINITY);
        let mut hi = Vector::splat(f32::NEG_INFINITY);
        for point in points {
            let local = Vector::new(point[0], point[1], point[2]) * JKA_TO_RAPIER;
            lo = lo.min(local);
            hi = hi.max(local);
        }
        if !lo.is_finite() || !hi.is_finite() {
            return None;
        }
        let half = ((hi - lo) * 0.5).max(Vector::splat(0.5 * JKA_TO_RAPIER));
        let centre = (hi + lo) * 0.5;
        let mass = (8.0 * half.x * half.y * half.z * TISSUE_DENSITY).clamp(0.2, 15.0);
        let body = self.bodies.insert(
            RigidBodyBuilder::dynamic()
                .pose(pose.clone())
                .linvel(linvel)
                .angvel(angvel)
                .linear_damping(if self.config.joint_model == 1 { 0.02 } else { 0.8 })
                .angular_damping(if self.config.joint_model == 1 { 0.05 } else { 4.5 })
                .ccd_enabled(self.config.ccd)
                .can_sleep(self.config.sleeping)
                .additional_solver_iterations(8),
        );
        self.previous_body_poses.insert(body, pose);
        self.colliders.insert_with_parent(
            ColliderBuilder::cuboid(half.x, half.y, half.z)
                .translation(centre)
                .mass(mass)
                .friction(0.9)
                .restitution(0.0)
                .collision_groups(groups),
            body,
            &mut self.bodies,
        );
        Some(DrivenBone { bone_index: template.bone_index, body, spawn_model_rigid: template.spawn_model_rigid })
    }
}

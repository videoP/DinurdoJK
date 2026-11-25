//! Bodies gore.
use crate::cgame::player_presenter::{
    assets::loading::{resolved_stage, stage_material},
    blend_for_alpha, model_bolt_matrix_timed, transform_jka_model_point,
    transform_jka_model_vector, Arc, DynamicModelSurface, DynamicWireframeClass, Ghoul2GpuSkinning,
    PlayerFxRequest, PlayerPresenter, PlayerSurfaceAsset, PresentationEvent, SaberDefinition,
    StageFrame, Vec3, MASK_PLAYERSOLID, MAX_CLIENTS, MAX_SABER_BLADES,
};
use crate::cgame::player_gore::{impact_position, project_gore, GpuSnap, PendingGore};
use crate::ninja_blood::{pain_hit_scale, SMALL_HIT_SCALE};
use jka_movement::{TraceQuery, TraceResult, TraceWorld, ENTITY_NONE, ENTITY_WORLD};

const SABER_COMPENSATION_MS: i32 = 400;

pub(in crate::cgame::player_presenter) struct SaberGoreHit {
    pub target: u16,
    time: i32,
    flesh_hit_time: Option<i32>,
    sequence: u64,
    skin_scale: f32,
    next_trace: [[i32; MAX_SABER_BLADES]; 2],
}

pub(in crate::cgame::player_presenter) struct SaberGoreBlade {
    attacker: u16,
    base: [f32; 3],
    tip: [f32; 3],
    shader: String,
    seed: u64,
}

impl PlayerPresenter {
    pub(in crate::cgame::player_presenter) fn note_saber_gore_hit(
        &mut self, event: &PresentationEvent, now: i32,
    ) {
        if !self.gore_marks_enabled() || self.assets.jk2_version().is_some() {
            return;
        }
        // jaPRO EV_SABER_HIT starts the compensation window on the owner.
        // eventParm selects the impact effect, including non-flesh Ghoul2.
        let entity = |field| {
            event.state.field_i32(field).filter(|number| (0..ENTITY_WORLD).contains(number))
                .map(|number| number as u16)
        };
        let (Some(attacker), Some(target)) = (entity("otherEntityNum2"), entity("otherEntityNum")) else {
            return;
        };
        if attacker == target {
            return;
        }
        devprintln!(2, "SABER GORE: hit attacker={attacker} target={target} parm={}", event.parm);
        let time = now.max(event.server_time);
        let flesh_hit_time = (event.parm != 0).then_some(event.server_time);
        let hit = self.saber_gore_hits.entry(attacker).or_insert(SaberGoreHit {
            target,
            time,
            flesh_hit_time,
            sequence: event.receive_sequence,
            skin_scale: SMALL_HIT_SCALE,
            next_trace: [[i32::MIN; MAX_SABER_BLADES]; 2],
        });
        hit.target = target;
        hit.time = time;
        hit.flesh_hit_time = flesh_hit_time;
        hit.sequence = event.receive_sequence;
        hit.skin_scale = SMALL_HIT_SCALE;
    }

    pub(crate) fn size_skin_gore_hits(&mut self) {
        // Match after dispatch so pain and its weapon hit can arrive in either order.
        for (victim, time, health) in self.skin_gore_pains.drain(..) {
            let mark = self.pending_gore.iter_mut().rev().find(|hit| {
                hit.entity == victim && hit.time == time && hit.skin_scale == SMALL_HIT_SCALE
            });
            let saber = self.saber_gore_hits.values_mut()
                .filter(|hit| {
                    hit.target == victim && hit.flesh_hit_time == Some(time)
                        && hit.skin_scale == SMALL_HIT_SCALE
                })
                .max_by_key(|hit| hit.sequence);
            let scale = pain_hit_scale(health);
            match (mark, saber) {
                (Some(mark), Some(saber)) if saber.sequence > mark.sequence => saber.skin_scale = scale,
                (Some(mark), _) => mark.skin_scale = scale,
                (None, Some(saber)) => saber.skin_scale = scale,
                (None, None) => {}
            }
        }
    }

    pub(in crate::cgame::player_presenter) fn expire_saber_gore_hits(&mut self, now: i32) {
        self.saber_gore_hits.retain(|_, hit| now.saturating_sub(hit.time) <= SABER_COMPENSATION_MS);
    }

    pub(in crate::cgame::player_presenter) fn queue_saber_gore_blade(
        &mut self,
        attacker: u16,
        saber: &SaberDefinition,
        (saber_num, blade_num): (usize, usize),
        base: [f32; 3],
        direction: [f32; 3],
        length: f32,
        now: i32,
    ) {
        let Some(hit) = self.saber_gore_hits.get_mut(&attacker) else { return; };
        // Skip the event frame, as CG_SaberCompWork does, and keep the
        // per-blade 5 ms debounce for high render frame rates.
        if !(1..=SABER_COMPENSATION_MS).contains(&now.saturating_sub(hit.time))
            || length <= 0.0 || saber.blade_hidden(blade_num)
        {
            return;
        }
        let Some(next_trace) = hit.next_trace.get_mut(saber_num)
            .and_then(|blades| blades.get_mut(blade_num)) else { return; };
        if *next_trace >= now { return; }
        *next_trace = now.saturating_add(5);

        let shader = if saber.blade_style2_start > 0 && blade_num >= saber.blade_style2_start {
            saber.g2_marks_shader2.as_deref()
        } else {
            saber.g2_marks_shader.as_deref()
        }.unwrap_or("gfx/damage/saberglowmark");
        let direction = Vec3::from_array(direction).normalize_or_zero();
        // CG_AddSaberBlade extends the contact trace one unit beyond the tip.
        let tip = Vec3::from_array(base) + direction * (length + 1.0);
        let seed = hit.sequence ^ u64::from(now as u32).rotate_left(17)
            ^ ((saber_num * MAX_SABER_BLADES + blade_num) as u64);
        self.saber_gore_blades.push(SaberGoreBlade {
            attacker, base, tip: tip.to_array(), shader: shader.to_owned(), seed,
        });
    }

    pub(crate) fn finish_saber_gore(&mut self, now: i32, draws: &[DynamicModelSurface]) {
        if self.saber_gore_blades.is_empty() { return; }
        let Some(world) = self.collision_world.as_mut() else {
            self.saber_gore_blades.clear();
            return;
        };
        let mut trace = crate::net::PredictionWorld {
            world, solids: &self.presentation_solids, client_num: self.viewer_client,
        };
        for blade in self.saber_gore_blades.drain(..) {
            let Some(hit) = self.saber_gore_hits.get(&blade.attacker) else { continue; };
            if !draws.iter().any(|draw| draw.entity_num == hit.target && draw.ghoul2_gpu.is_some()) {
                devprintln!(2, "SABER GORE: target {} has no GPU-skinned draw this frame", hit.target);
                continue;
            }
            let Some(snaps) = self.last_gpu.get(&hit.target) else {
                devprintln!(2, "SABER GORE: target {} has no GPU snapshot", hit.target);
                continue;
            };
            // CG_AddSaberBlade truncates the blade at solid architecture.
            let blade_trace = trace.trace(TraceQuery {
                start: blade.base, end: blade.tip, mins: [0.0; 3], maxs: [0.0; 3],
                pass_entity: ENTITY_NONE, mask: 0x0000_0001 | 0x0000_1000,
            });
            let base = Vec3::from_array(blade.base);
            let mut tip = Vec3::from_array(blade_trace.end);
            if blade_trace.fraction != 1.0 {
                tip += (tip - base).normalize_or_zero() * 0.1;
            }
            // CG_SaberCompWork traces base -> tip only: jaPRO leaves the
            // backward pass disabled ("sometimes it just makes too many
            // effects"). Confirm the authoritative target against its posed
            // triangles before queuing either mark mode.
            let (start, end) = (base, tip);
            let contact = trace.trace(TraceQuery {
                start: start.to_array(), end: end.to_array(), mins: [0.0; 3], maxs: [0.0; 3],
                pass_entity: i32::from(blade.attacker), mask: MASK_PLAYERSOLID,
            });
            if contact.entity != i32::from(hit.target) {
                let target_solid = self.presentation_solids.iter()
                    .find(|solid| solid.number == i32::from(hit.target))
                    .map(|solid| format!("{:?}", solid.clip));
                devprintln!(
                    2,
                    "SABER GORE: contact trace hit entity {} not target {} (solids={} target_clip={:?} seg={:?}->{:?})",
                    contact.entity, hit.target, self.presentation_solids.len(), target_solid,
                    start.to_array(), end.to_array(),
                );
                continue;
            }
            let Some(position) = impact_position(snaps, start.to_array(), end.to_array()) else {
                devprintln!(2, "SABER GORE: blade segment misses the posed mesh of {}", hit.target);
                continue;
            };
            devprintln!(2, "SABER GORE: mark queued on {} skin_gore={}", hit.target, self.skin_gore_enabled);
            let center = Vec3::from_array(position);
            let direction = (end - center).normalize_or_zero();
            if direction == Vec3::ZERO { continue; }
            let seed = blade.seed;
            let random = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15);
            let size = 3.0 + (random >> 40) as f32 / 16_777_216.0;
            if self.skin_gore_enabled {
                self.skin_gore.add_damage(hit.target, snaps, position, size * hit.skin_scale, 1.0);
                continue;
            }
            self.pending_gore.push(PendingGore {
                entity: hit.target, start: position, end: (center + direction * 4.0).to_array(),
                trace: false, size, skin_scale: 1.0,
                life_ms: 5_000 + (random % 5_001) as i32, shader: blade.shader.clone(),
                time: now, sequence: seed,
            });
        }
        let excess = self.pending_gore.len().saturating_sub(256);
        self.pending_gore.drain(..excess);
    }

    /// `CG_VehMuzzleFireFX`: the muzzle effect of every muzzle named in the
    /// event's bit mask (`trickedentindex`), at that muzzle's bolt on the vehicle.
    pub(in crate::cgame::player_presenter) fn vehicle_muzzle_fire(
        &mut self,
        event: &PresentationEvent,
    ) {
        let state = &event.state;
        let Ok(owner) = u16::try_from(state.field_i32("owner").unwrap_or(-1)) else {
            return;
        };
        let mask = state.field_i32("trickedentindex").unwrap_or(0);
        let Some(snap) = self.vehicle_snaps.get(&owner).cloned() else {
            return;
        };
        let Some(definition) = self.vehicle_definitions.get(&snap.vehicle) else {
            return;
        };
        let mut requests = Vec::new();
        for (index, weapon) in definition.weap_muzzles.iter().enumerate() {
            let Some(weapon) = weapon else { continue };
            if mask & (1 << index) == 0 {
                continue;
            }
            let Some(effect) = self.vehicle_definitions.weapon_muzzle_fx(weapon) else {
                continue;
            };
            let number = index + 1;
            let bolt = [format!("*muzzle{number}"), format!("*flash{number}")]
                .into_iter()
                .find_map(|tag| {
                    model_bolt_matrix_timed(
                        &mut self.perf,
                        &snap.model.glm,
                        &snap.model.gla,
                        &snap.pose,
                        &tag,
                    )
                    .ok()
                    .flatten()
                });
            let Some(m) = bolt else { continue };
            let origin =
                transform_jka_model_point([m[0][3], m[1][3], m[2][3]], snap.axis, snap.origin);
            // NEGATIVE_Y of the bolt matrix is the muzzle direction.
            let dir = transform_jka_model_vector([-m[0][1], -m[1][1], -m[2][1]], snap.axis);
            requests.push(PlayerFxRequest::EffectDir {
                name: effect.to_owned(),
                origin,
                dir,
            });
        }
        self.fx_requests.extend(requests);
    }

    /// `CG_G2MarkEvent`: burn a decal onto the hit player's model.
    pub(in crate::cgame::player_presenter) fn add_gore_mark(&mut self, event: &PresentationEvent) {
        const WP_BRYAR_PISTOL: i32 = 4;
        const WP_BLASTER: i32 = 5;
        const WP_DISRUPTOR: i32 = 6;
        const WP_BOWCASTER: i32 = 7;
        const WP_REPEATER: i32 = 8;
        const WP_ROCKET_LAUNCHER: i32 = 11;
        const WP_THERMAL: i32 = 12;
        const WP_CONCUSSION: i32 = 15;
        const WP_BRYAR_OLD: i32 = 16;
        const WP_TURRET: i32 = 18;
        if !self.gore_marks_enabled() {
            return;
        }
        let state = &event.state;
        let target = state.field_i32("otherEntityNum").unwrap_or(-1);
        if !(0..ENTITY_WORLD).contains(&target) {
            return;
        }
        let (size, shader) = match state.field_i32("weapon").unwrap_or(0) {
            WP_BRYAR_PISTOL | WP_CONCUSSION | WP_BRYAR_OLD | WP_BLASTER | WP_DISRUPTOR
            | WP_BOWCASTER | WP_REPEATER | WP_TURRET => (4.0, "gfx/damage/bodyburnmark1"),
            WP_ROCKET_LAUNCHER | WP_THERMAL => (24.0, "gfx/damage/bodybigburnmark1"),
            _ => return,
        };
        let Some(start) = crate::cgame::entity_vec3(state, "origin") else { return; };
        let Some(end) = crate::cgame::entity_vec3(state, "origin2") else { return; };
        // Events may precede the player's first presented skinned frame (also
        // true after teleport/demo seeks). Do not throw a valid server mark away.
        self.pending_gore.push(PendingGore {
            entity: target as u16, start, end,
            trace: event.parm != 0, size, skin_scale: SMALL_HIT_SCALE, shader: shader.to_owned(),
            life_ms: 10_000 + (event.receive_sequence.wrapping_mul(7919) % 10_001) as i32,
            time: event.server_time, sequence: event.receive_sequence,
        });
        if self.pending_gore.len() > 256 { self.pending_gore.remove(0); }
    }

    /// CG_G2Trace: world trace, then CG_ClipMoveToEntities with g2Check over
    /// `presentation_solids`. `None` only without a collision world.
    fn cg_g2_trace(&mut self, query: TraceQuery) -> Option<TraceResult> {
        let world = self.collision_world.as_mut()?;
        let mut result = world.trace(query);
        result.entity = if result.fraction < 1.0 { ENTITY_WORLD } else { ENTITY_NONE };
        for solid in &self.presentation_solids {
            if solid.number == query.pass_entity || solid.skip_movement
                || (solid.number > MAX_CLIENTS as i32
                    && solid.generic_enemy_index - (ENTITY_NONE + 1) == self.viewer_client)
            {
                continue;
            }
            let candidate = world.trace_entity(query, solid.clip);
            let previous = result;
            if candidate.all_solid != 0 || candidate.fraction < result.fraction {
                result = candidate;
                result.entity = solid.number;
            } else if candidate.start_solid != 0 {
                result.start_solid = 1;
                result.entity = solid.number;
            }
            if result.all_solid != 0 {
                break;
            }
            // CG_ClipMoveToEntities keeps the selected hull result when
            // CG_G2TraceCollide succeeds. A mesh miss restores the previous
            // result and lets the remaining solids compete for the trace.
            if candidate.fraction < 1.0 || candidate.start_solid != 0 {
                if let Some(snaps) = u16::try_from(solid.number).ok()
                    .and_then(|entity| self.last_gpu.get(&entity))
                    .filter(|snaps| !snaps.is_empty())
                {
                    if impact_position(snaps, query.start, query.end).is_none() {
                        result = previous;
                    }
                }
            }
        }
        Some(result)
    }

    fn gore_projection_start(&mut self, hit: &PendingGore) -> Option<[f32; 3]> {
        if !hit.trace {
            return Some(hit.start);
        }
        if self.collision_world.is_none() {
            return impact_position(self.last_gpu.get(&hit.entity)?, hit.start, hit.end);
        }
        let mut query = TraceQuery {
            start: hit.start, end: hit.end, mins: [0.0; 3], maxs: [0.0; 3],
            mask: MASK_PLAYERSOLID, pass_entity: ENTITY_NONE,
        };
        let target = i32::from(hit.entity);
        let first = self.cg_g2_trace(query)?;
        if first.entity == target || first.entity >= ENTITY_WORLD {
            return Some(first.end);
        }
        query.pass_entity = first.entity;
        let second = self.cg_g2_trace(query)?;
        (second.entity == target).then_some(second.end)
    }

    /// FX_GHOUL2_DECALS / SFxHelper::AddGhoul2Decal. Legacy marks follow
    /// jaPRO C_G2Mark; SkinGore keeps its posed-surface stamp.
    pub(crate) fn add_authored_ghoul2_decals(
        &mut self,
        decals: &[crate::fx::system::FxDecalSpawn],
        entities: &[crate::cgame::PresentedEntity],
        now: i32,
    ) {
        if decals.is_empty() || !self.gore_marks_enabled() { return; }
        if self.skin_gore_enabled {
            for decal in decals {
                let start = std::array::from_fn(|i| decal.origin[i] + decal.normal[i] * 8.0);
                let end = std::array::from_fn(|i| decal.origin[i] - decal.normal[i] * 16.0);
                // Pick only an actual skinned-surface intersection, never the
                // nearest player through a wall or from pure proximity.
                let nearest = self.last_gpu.iter().filter_map(|(&entity, snaps)| {
                    let hit = impact_position(snaps, start, end)?;
                    let d2 = hit.iter().zip(decal.origin).map(|(a,b)|(a-b)*(a-b)).sum::<f32>();
                    Some((entity, d2, hit))
                }).min_by(|a,b| a.1.total_cmp(&b.1));
                if let Some((entity, _, hit)) = nearest {
                    if let Some(snaps) = self.last_gpu.get(&entity) {
                        self.skin_gore.add_damage(entity, snaps, hit, decal.radius, 1.0);
                    }
                }
            }
            return;
        }
        // C_G2Mark (cg_main.c): CG_G2Trace from the decal origin 64 units
        // along the effect's forward axis, from ENTITYNUM_NONE with
        // MASK_PLAYERSOLID. World geometry blocks it. The first Ghoul2 entity
        // it reaches gets CG_AddGhoul2Mark(size, tr.endpos -> end,
        // Q_irand(2000, 4000)), sharing the cg_ghoul2Marks limit in append_gore.
        crate::net::solid_entities_into(entities, now, &mut self.presentation_solids);
        for (index, decal) in decals.iter().enumerate() {
            let end: [f32; 3] = std::array::from_fn(|i| decal.origin[i] + decal.normal[i] * 64.0);
            let Some(trace) = self.cg_g2_trace(TraceQuery {
                start: decal.origin, end, mins: [0.0; 3], maxs: [0.0; 3],
                pass_entity: ENTITY_NONE, mask: MASK_PLAYERSOLID,
            }) else { return; };
            if !(0..ENTITY_WORLD).contains(&trace.entity) { continue; }
            let entity = trace.entity as u16;
            if !self.last_gpu.get(&entity).is_some_and(|snaps| !snaps.is_empty()) { continue; }
            let sequence = (u64::from(now as u32) << 24) ^ (u64::from(entity) << 12) ^ index as u64;
            let random = sequence.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 33;
            self.pending_gore.push(PendingGore {
                entity, start: trace.end, end, trace: false, size: decal.radius, skin_scale: 1.0,
                life_ms: 2_000 + (random % 2_001) as i32, shader: decal.shader.clone(),
                time: now, sequence,
            });
        }
        let excess = self.pending_gore.len().saturating_sub(256);
        self.pending_gore.drain(..excess);
    }

    /// Draw this entity's burn marks through the same GPU skinning as its body.
    pub(in crate::cgame::player_presenter) fn append_gore(
        &mut self,
        entity: u16,
        now: i32,
        body_surfaces: &[PlayerSurfaceAsset],
        body_lod: usize,
        draws: &mut Vec<DynamicModelSurface>,
    ) {
        self.gore.retain(|gore| now - gore.start_time < gore.life_ms);
        // Now that a GPU skin snapshot is available, resolve pending hit rays
        // against the same skinned geometry used to render this frame.
        let mut hits = Vec::new();
        let have_snapshot = self.last_gpu.get(&entity).is_some_and(|snaps| !snaps.is_empty());
        self.pending_gore.retain(|hit| {
            if now.saturating_sub(hit.time) > 1500 { return false; }
            if hit.entity == entity && hit.time <= now && have_snapshot {
                hits.push(hit.clone());
                false
            } else { true }
        });
        let hits = hits.into_iter().filter_map(|hit| {
            let start = self.gore_projection_start(&hit)?;
            let distance_sq: f32 = hit.end.iter().zip(start)
                .map(|(end, start)| (end - start).powi(2)).sum();
            (distance_sq.is_finite() && distance_sq >= 0.01).then_some((hit, start))
        }).collect::<Vec<_>>();
        if self.skin_gore_enabled {
            if let Some(snaps) = self.last_gpu.get(&entity) {
                for (hit, start) in hits {
                    // Keep the trace decision shared with Legacy, but place
                    // the spherical UV stamp on the posed surface if available.
                    let position = impact_position(snaps, start, hit.end).unwrap_or(start);
                    self.skin_gore.add_damage(entity, snaps, position, hit.size * hit.skin_scale, 1.0);
                }
            }
            let Some(snaps) = self.last_gpu.get(&entity) else { return; };
            let mut overlays = Vec::new();
            for snap in snaps {
                let Some(map) = self.skin_gore.get(entity, &snap.mesh_key) else { continue; };
                let Some((body, skin)) = draws.iter().find_map(|body| {
                    let skin = body.ghoul2_gpu.as_ref()?;
                    (skin.mesh_key == snap.mesh_key).then_some((body, skin))
                }) else { continue; };
                let Some(assets) = crate::cgame::skin_gore::ensure_assets(&mut self.assets) else {
                    return;
                };
                let mut overlay = body.clone();
                let mut overlay_skin = skin.clone();
                overlay_skin.skin_gore = Some(crate::renderer::Ghoul2SkinGore {
                    key: Arc::from(format!("{entity}#{}", skin.mesh_key)),
                    pixels: Arc::clone(&map.pixels), revision: map.revision,
                });
                overlay_skin.source_material = None;
                overlay_skin.color = [1.0, 1.0, 1.0, 1.0];
                overlay_skin.uv_xform = [1.0, 1.0, 0.0, 0.0];
                overlay_skin.uv_cross = [0.0;4];
                overlay.ghoul2_gpu = Some(overlay_skin);
                overlay.texture = Some(Arc::clone(&assets.albedo));
                overlay.alpha_mode = crate::renderer::DynamicModelAlphaMode::Mask;
                overlay.rt_skinned_key = None;
                overlay.authored_glow = false;
                overlays.push(overlay);
            }
            draws.extend(overlays);
            return;
        }
        if let Some(snaps) = self.last_gpu.get(&entity) {
            // G2API_AddSkinGore records each LOD at impact time. Project onto
            // alternate meshes with the same impact pose and global bone indices.
            let mut alternate_lods = Vec::new();
            if !hits.is_empty() {
                for surface in body_surfaces {
                    let Some(mesh) = surface.gpu_meshes.get(body_lod).and_then(Option::as_ref) else {
                        continue;
                    };
                    let Some(snap) = snaps.iter().find(|snap| {
                        snap.mesh_key == mesh.base_key || snap.mesh_key == mesh.key
                    }) else { continue; };
                    for (lod, mesh) in surface.gpu_meshes.iter().enumerate() {
                        let Some(mesh) = mesh else { continue; };
                        // Promoted and original topology can swap when the
                        // user's jiggle path changes, including at this LOD.
                        if lod != body_lod || snap.mesh_key != mesh.base_key {
                            alternate_lods.push(GpuSnap {
                                mesh_key: Arc::clone(&mesh.base_key),
                                vertices: Arc::clone(&mesh.base_vertices),
                                indices: Arc::clone(&mesh.cpu_indices),
                                ..snap.clone()
                            });
                        }
                        if mesh.key != mesh.base_key && snap.mesh_key != mesh.key {
                            alternate_lods.push(GpuSnap {
                                mesh_key: Arc::clone(&mesh.key),
                                vertices: Arc::clone(&mesh.vertices),
                                indices: Arc::clone(&mesh.indices),
                                ..snap.clone()
                            });
                        }
                    }
                }
            }
            for (hit, start) in hits {
                let active_records: usize = self.gore.iter().filter(|g| g.entity == entity)
                    .flat_map(|g| &g.surfaces)
                    .filter(|surface| snaps.iter().any(|snap| snap.mesh_key == surface.base_key))
                    .count();
                if active_records >= self.gore_limit {
                    devprintln!(2, "GORE: entity {entity} at cg_ghoul2Marks limit {}", self.gore_limit);
                    break;
                }
                let id = self.gore_next_id;
                self.gore_next_id = self.gore_next_id.wrapping_add(1);
                let mut surfaces = project_gore(snaps, id, start, hit.end, hit.size, hit.sequence);
                surfaces.extend(project_gore(&alternate_lods, id, start, hit.end, hit.size, hit.sequence));
                if surfaces.is_empty() {
                    devprintln!(2, "GORE: projection of {} onto entity {entity} produced no triangles", hit.shader);
                    continue;
                }
                self.gore.push(crate::cgame::player_gore::Gore {
                    entity, start_time: now,
                    life_ms: hit.life_ms,
                    fade_out_ms: 0,
                    shader: hit.shader, surfaces,
                });
            }
        }
        let Some(snaps) = self.last_gpu.get(&entity) else { return; };
        let marks: Vec<_> = self
            .gore
            .iter()
            .filter(|gore| gore.entity == entity && now >= gore.start_time)
            .filter_map(|gore| {
                let surfaces = gore.surfaces.iter()
                    .filter(|surface| snaps.iter().any(|snap| snap.mesh_key == surface.base_key))
                    .cloned()
                    .collect::<Vec<_>>();
                (!surfaces.is_empty()).then(|| (
                    gore.shader.clone(),
                    gore.life_ms - (now - gore.start_time),
                    gore.fade_out_ms,
                    surfaces,
                ))
            })
            .collect();
        let mut extra = Vec::new();
        for (shader, remaining, fade_out_ms, surfaces) in marks {
            let fade = if fade_out_ms > 0 {
                (remaining as f32 / fade_out_ms as f32).clamp(0.0, 1.0)
            } else { 1.0 };
            let (texture, alpha_mode, _) = self.custom_shader_material(&shader);
            let shaders = Arc::clone(&self.shaders);
            let effective = self.shader_remaps.effective_name(&shader).into_owned();
            let authored = jka_assets::shader::find_shader(&shaders, &effective);
            let seconds = jka_assets::shader::shader_time(
                now as f32 * 0.001, self.shader_remaps.time_offset_for(&effective),
                authored.and_then(|shader| shader.clamp_time),
            );
            let stages = authored.filter(|_| fade_out_ms == 0).into_iter()
                .flat_map(|shader| &shader.stages)
                .filter(|stage| !stage.image.is_empty() && !stage.image.starts_with('$'))
                .map(|stage| {
                    let (texture, mode) = self.load_stage_texture(&stage_material(stage));
                    resolved_stage(stage, mode, texture)
                }).collect::<Vec<_>>();
            for surface in surfaces {
                let Some(body) = draws.iter().find(|draw| {
                    draw.ghoul2_gpu
                        .as_ref()
                        .is_some_and(|skin| *skin.mesh_key == *surface.base_key)
                }) else {
                    continue;
                };
                let Some(skin) = body.ghoul2_gpu.as_ref() else {
                    continue;
                };
                let color = [1.0, 1.0, 1.0, fade];
                let mark = DynamicModelSurface {
                    geometry_source: crate::renderer::ModelGeometrySource::Ghoul2,
                    authored_lighting: false,
                    material_trace: None,
                    entity_num: body.entity_num,
                    wireframe_class: DynamicWireframeClass::Player,
                    raster_visible: body.raster_visible, portal_only: body.portal_only,
                    vertices: Arc::clone(&body.vertices),
                    indices: Arc::clone(&surface.indices),
                    lighting_origin: body.lighting_origin,
                    rt_rigid: None,
                    rt_skinned_key: None,
                    ghoul2_gpu: Some(Ghoul2GpuSkinning { skin_gore: None, foliage_wind: false,
                        mesh_key: Arc::clone(&surface.key),
                        vertices: Arc::clone(&surface.vertices),
                        indices: Arc::clone(&surface.indices),
                        bones: Arc::clone(&skin.bones),
                        heat_bone: skin.heat_bone,
                        axis: skin.axis,
                        origin: skin.origin,
                        color,
                        uv_xform: [1.0, 1.0, 0.0, 0.0],
                        uv_cross: [0.0; 4],
                        specular: None,
                        bulge_height: skin.bulge_height,
                        env_map: false,
                        jiggle_offsets: skin.jiggle_offsets,
                        // Gore surfaces are JKA-only: dismemberment keys off JKA
                        // surface names, which Source surfaces never match.
                        source_material: None,
                    }),
                    cloth_gpu: None, fx_gpu_sprites: None,
                    texture: texture.clone(),
                    alpha_mode: blend_for_alpha(alpha_mode, fade),

                    authored_glow: false,
                };
                if stages.is_empty() {
                    if mark.texture.is_some() || fade_out_ms > 0 { extra.push(mark); }
                } else {
                    let frame = StageFrame {
                        axis: skin.axis, origin: skin.origin, viewer: self.stage_view_position,
                    };
                    for stage in &stages {
                        if stage.texture.is_none() { continue; }
                        let mut pass = mark.clone();
                        Self::apply_stage(&mut pass, stage, fade, seconds, &frame);
                        extra.push(pass);
                    }
                }
            }
        }
        draws.extend(extra);
    }
}

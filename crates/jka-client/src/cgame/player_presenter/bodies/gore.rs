//! Bodies gore.
use crate::cgame::player_presenter::{
    blend_for_alpha, model_bolt_matrix_timed, transform_jka_model_point,
    transform_jka_model_vector, Arc, DynamicModelSurface, DynamicWireframeClass, Ghoul2GpuSkinning,
    PlayerFxRequest, PlayerPresenter, PresentationEvent,
};

impl PlayerPresenter {
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
        const WP_SABER: i32 = 3;
        const WP_BRYAR_PISTOL: i32 = 4;
        const WP_BLASTER: i32 = 5;
        const WP_DISRUPTOR: i32 = 6;
        const WP_BOWCASTER: i32 = 7;
        const WP_REPEATER: i32 = 8;
        const WP_ROCKET_LAUNCHER: i32 = 11;
        const WP_THERMAL: i32 = 12;
        const WP_CONCUSSION: i32 = 15;
        const WP_BRYAR_OLD: i32 = 16;
        const WP_EMPLACED_GUN: i32 = 17;
        const WP_TURRET: i32 = 18;
        if self.gore_limit == 0 {
            return;
        }
        let state = &event.state;
        let Ok(target) = u16::try_from(state.field_i32("otherEntityNum").unwrap_or(-1)) else {
            return;
        };
        let (size, shader) = match state.field_i32("weapon").unwrap_or(0) {
            WP_BRYAR_PISTOL | WP_CONCUSSION | WP_BRYAR_OLD | WP_BLASTER | WP_DISRUPTOR
            | WP_BOWCASTER | WP_REPEATER | WP_EMPLACED_GUN | WP_TURRET => (4.0, "gfx/damage/bodyburnmark1"),
            WP_SABER => (6.0, "gfx/damage/bodyburnmark1"),
            WP_ROCKET_LAUNCHER | WP_THERMAL => (24.0, "gfx/damage/bodybigburnmark1"),
            _ => return,
        };
        let origin = crate::cgame::entity_vec3(state, "origin").unwrap_or(event.position);
        let predicted = crate::cgame::entity_vec3(state, "origin2").unwrap_or(origin);
        // Events may precede the player's first presented skinned frame (also
        // true after teleport/demo seeks). Do not throw a valid server mark away.
        self.pending_gore.push(crate::cgame::player_gore::PendingGore {
            entity: target, start: origin, end: predicted,
            trace: event.parm != 0, size, shader: shader.to_owned(),
            time: event.server_time, sequence: event.receive_sequence,
        });
        if self.pending_gore.len() > 256 { self.pending_gore.remove(0); }
    }

    /// FX_GHOUL2_DECALS / SFxHelper::AddGhoul2Decal: project the authored
    /// impact shader onto a nearby player's actual GPU-skinned triangles.
    /// The short ray must hit skinned geometry: mere proximity to a player
    /// must never paint them through BSP walls.
    pub(crate) fn add_authored_ghoul2_decals(&mut self, decals: &[crate::fx::system::FxDecalSpawn], now: i32) {
        if self.gore_limit == 0 { return; }
        for decal in decals {
            let normal = decal.normal;
            let start = std::array::from_fn(|i| decal.origin[i] + normal[i] * 8.0);
            let end = std::array::from_fn(|i| decal.origin[i] - normal[i] * 16.0);
            let mut nearest: Option<(u16, f32, Vec<crate::cgame::player_gore::GoreSurface>)> = None;
            for (&entity, snaps) in &self.last_gpu {
                if snaps.is_empty() { continue; }
                // Cheap coarse rejection before per-triangle skinning; model
                // origins are typically at the feet, so leave headroom.
                if !snaps[0].origin.iter().zip(decal.origin.iter()).all(|(a,b)| (a-b).abs() < 150.0) { continue; }
                let id = self.gore_next_id;
                let surfaces = crate::cgame::player_gore::build_gore(snaps, id, start, end, true, decal.radius);
                if surfaces.is_empty() { continue; }
                let sq = snaps[0].origin.iter().zip(decal.origin.iter()).map(|(a,b)| (a-b)*(a-b)).sum::<f32>();
                if nearest.as_ref().is_none_or(|(_, dist, _)| sq < *dist) {
                    nearest = Some((entity, sq, surfaces));
                }
            }
            if let Some((entity, _, surfaces)) = nearest {
                self.gore_next_id = self.gore_next_id.wrapping_add(1);
                while self.gore.iter().filter(|g| g.entity == entity).count() >= self.gore_limit {
                    if let Some(i) = self.gore.iter().position(|g| g.entity == entity) { self.gore.remove(i); } else { break; }
                }
                self.gore.push(crate::cgame::player_gore::Gore {
                    entity, start_time: now, life_ms: decal.life_ms,
                    shader: decal.shader.clone(), surfaces,
                });
            }
        }
    }

    /// Draw this entity's burn marks through the same GPU skinning as its body.
    pub(in crate::cgame::player_presenter) fn append_gore(
        &mut self,
        entity: u16,
        now: i32,
        draws: &mut Vec<DynamicModelSurface>,
    ) {
        self.gore.retain(|gore| now - gore.start_time < gore.life_ms);
        // Now that a GPU skin snapshot is available, resolve pending hit rays
        // against the same skinned geometry used to render this frame.
        let mut hits = Vec::new();
        let have_snapshot = self.last_gpu.contains_key(&entity);
        self.pending_gore.retain(|hit| {
            if now.saturating_sub(hit.time) > 1500 { return false; }
            if hit.entity == entity && hit.time <= now && have_snapshot {
                hits.push(hit.clone());
                false
            } else { true }
        });
        if let Some(snaps) = self.last_gpu.get(&entity) {
            for hit in hits {
                let id = self.gore_next_id;
                self.gore_next_id = self.gore_next_id.wrapping_add(1);
                let surfaces = crate::cgame::player_gore::build_gore(
                    snaps, id, hit.start, hit.end, hit.trace, hit.size,
                );
                if surfaces.is_empty() { continue; }
                while self.gore.iter().filter(|g| g.entity == entity).count() >= self.gore_limit {
                    let Some(oldest) = self.gore.iter().position(|g| g.entity == entity) else { break; };
                    self.gore.remove(oldest);
                }
                self.gore.push(crate::cgame::player_gore::Gore {
                    entity, start_time: hit.time,
                    life_ms: 10_000 + (hit.sequence.wrapping_mul(7919) % 10_001) as i32,
                    shader: hit.shader, surfaces,
                });
            }
        }
        let marks: Vec<_> = self
            .gore
            .iter()
            .filter(|gore| gore.entity == entity && now >= gore.start_time)
            .map(|gore| {
                (
                    gore.shader.clone(),
                    gore.life_ms - (now - gore.start_time),
                    gore.surfaces.clone(),
                )
            })
            .collect();
        let mut extra = Vec::new();
        for (shader, remaining, surfaces) in marks {
            let fade = if remaining < 1000 {
                remaining as f32 / 1000.0
            } else {
                1.0
            };
            let (texture, alpha_mode, _) = self.custom_shader_material(&shader);
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
                extra.push(DynamicModelSurface {
                    geometry_source: crate::renderer::ModelGeometrySource::Ghoul2,
                    authored_lighting: false,
                    material_trace: None,
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
                        uv_cross: [0.0; 4],
                        specular: None,
                        bulge_height: 0.0,
                        env_map: false,
                        jiggle_offsets: skin.jiggle_offsets,
                        // Gore surfaces are JKA-only: dismemberment keys off JKA
                        // surface names, which Source surfaces never match.
                        source_material: None,
                    }),
                    fx_gpu_sprites: None,
                    texture: texture.clone(),
                    alpha_mode: blend_for_alpha(alpha_mode, fade),

                    authored_glow: false,
                });
            }
        }
        draws.extend(extra);
    }
}

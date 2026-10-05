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
        let Ok(target) = u16::try_from(state.field_i32("otherEntityNum").unwrap_or(-1)) else {
            return;
        };
        let (size, shader) = match state.field_i32("weapon").unwrap_or(0) {
            WP_BRYAR_PISTOL | WP_CONCUSSION | WP_BRYAR_OLD | WP_BLASTER | WP_DISRUPTOR
            | WP_BOWCASTER | WP_REPEATER | WP_TURRET => (4.0, "gfx/damage/bodyburnmark1"),
            WP_ROCKET_LAUNCHER | WP_THERMAL => (24.0, "gfx/damage/bodybigburnmark1"),
            _ => return,
        };
        let Some(snaps) = self.last_gpu.get(&target) else {
            return;
        };
        let origin = crate::cgame::entity_vec3(state, "origin").unwrap_or(event.position);
        let predicted = crate::cgame::entity_vec3(state, "origin2").unwrap_or(origin);
        let id = self.gore_next_id;
        self.gore_next_id = self.gore_next_id.wrapping_add(1);
        let surfaces = crate::cgame::player_gore::build_gore(
            snaps,
            id,
            origin,
            predicted,
            event.parm != 0,
            size,
        );
        if surfaces.is_empty() {
            return;
        }
        while self
            .gore
            .iter()
            .filter(|gore| gore.entity == target)
            .count()
            >= self.gore_limit
        {
            let Some(oldest) = self.gore.iter().position(|gore| gore.entity == target) else {
                break;
            };
            self.gore.remove(oldest);
        }
        self.gore.push(crate::cgame::player_gore::Gore {
            entity: target,
            start_time: event.server_time,
            life_ms: 10_000 + (event.receive_sequence.wrapping_mul(7919) % 10_001) as i32,
            shader,
            surfaces,
        });
    }

    /// Draw this entity's burn marks through the same GPU skinning as its body.
    pub(in crate::cgame::player_presenter) fn append_gore(
        &mut self,
        entity: u16,
        now: i32,
        draws: &mut Vec<DynamicModelSurface>,
    ) {
        self.gore
            .retain(|gore| now - gore.start_time < gore.life_ms);
        let marks: Vec<_> = self
            .gore
            .iter()
            .filter(|gore| gore.entity == entity && now >= gore.start_time)
            .map(|gore| {
                (
                    gore.shader,
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
            let (texture, alpha_mode) = self.custom_shader_material(shader);
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
}

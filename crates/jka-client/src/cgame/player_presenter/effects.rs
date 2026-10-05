//! Effects.
use crate::cgame::player_presenter::{
    cosmetic_draws_for_mask, model_bolt_origin_timed, multiply_3x4, scene, AnimationSet, Arc,
    ClientGameState, DynamicModelAlphaMode, FxGpuSpriteInstance, Ghoul2Animator, HashMap,
    Matrix3x4, PlayerAngleEntity, PlayerFxRequest, PlayerModelAsset, PlayerPresenter,
    PresentedEntity, TextureData, TraceQuery, Vec3, ANIM_TOGGLEBIT, BLOB_SHADOW_DISTANCE,
    BLOB_SHADOW_DROID_RADIUS, BLOB_SHADOW_MAXS, BLOB_SHADOW_MINS, BLOB_SHADOW_RADIUS, CLASS_REMOTE,
    CLASS_SEEKER, DEFAULT_PLAYER_MAXS, DEFAULT_PLAYER_MINS, DEFAULT_VIEWHEIGHT, EF2_SHIP_DEATH,
    EF_BODYPUSH, EF_DEAD, EF_NODRAW, EF_TELEPORT_BIT, ET_NPC, ET_PLAYER, FORCE_LEVEL_2,
    FORCE_LEVEL_3, FP_GRIP, FP_PROTECT, FP_RAGE, MASK_PLAYERSOLID, MAX_GRIP_DISTANCE,
    PUSH_BONE_NAMES, PW_CLOAKED, PW_DISINT_4, TEAM_SPECTATOR,
};
use jka_movement::TraceWorld;

impl PlayerPresenter {
    /// jaPRO CG_PlayerSprites, called from CG_Player once the client is valid and
    /// not EF_NODRAW: at most one icon floats 48 units over the head - connection
    /// trouble first, else a voice command, else the talk balloon (never on NPCs).
    /// `visible` is false for the viewer's own body in first person, where
    /// CG_PlayerFloatSprite marks the sprite RF_THIRD_PERSON (mirrors only).
    pub fn queue_player_sprites(
        &mut self,
        entity: &PresentedEntity,
        game: &ClientGameState,
        current_time: i32,
        visible: bool,
    ) {
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
        let Some(shader) = head_sprite_shader(e_flags, voice) else {
            return;
        };
        let mut origin = entity.origin;
        origin[2] += 48.0;
        self.fx_requests
            .push(PlayerFxRequest::HeadSprite { origin, shader });
    }

    /// CG_Player's force-power effect block: drain/lightning effects at the
    /// hand bolts, push/pull (PW_DISINT_4) and grip puffs, and the
    /// EF_BODYPUSH full-body blur.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::cgame::player_presenter) fn queue_force_fx(
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
                    _ => Some(if wide {
                        "mp/drainwide_japro"
                    } else {
                        "mp/drain_japro"
                    }),
                }
                .map(|name| (name, "*l_hand"))
            } else {
                // BOTH_FORCE_2HANDEDLIGHTNING_HOLD alternates hands (Q_irand).
                // Force lightning itself is not governed by cg_drainFX.
                let two_handed =
                    animation_number("BOTH_FORCE_2HANDEDLIGHTNING_HOLD").is_some_and(|anim| {
                        state.field_i32("torsoAnim").unwrap_or(-1) & !0x800 == anim
                    });
                let hand = if two_handed && (current_time / 50) & 1 == 1 {
                    "*r_hand"
                } else {
                    "*l_hand"
                };
                Some((
                    if pass > FORCE_LEVEL_2 {
                        "force/lightningwide"
                    } else {
                        "force/lightning"
                    },
                    hand,
                ))
            };
            if let Some((name, hand)) = request {
                if let Some(origin) =
                    model_bolt_origin_timed(&mut self.perf, model, pose, axis, render_origin, hand)?
                {
                    self.fx_requests.push(PlayerFxRequest::Effect {
                        name,
                        origin,
                        axis: fx_axis,
                    });
                }
            }
        }
        if state.field_i32("eFlags").unwrap_or(0) & EF_BODYPUSH != 0 {
            for bone in PUSH_BONE_NAMES {
                if let Some(index) = Ghoul2Animator::bone_index(&model.gla, bone) {
                    let m = multiply_3x4(&pose[index], &model.gla.skeleton[index].base_pose);
                    let origin =
                        transform_jka_model_point([m[0][3], m[1][3], m[2][3]], axis, render_origin);
                    self.fx_requests.push(PlayerFxRequest::PushPuffs { origin });
                }
            }
        }
        if state.field_i32("powerups").unwrap_or(0) & (1 << PW_DISINT_4) != 0 {
            if let Some(origin) = model_bolt_origin_timed(
                &mut self.perf,
                model,
                pose,
                axis,
                render_origin,
                "*l_hand",
            )? {
                let gripping =
                    state.field_i32("forcePowersActive").unwrap_or(0) & (1 << FP_GRIP) != 0;
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
    pub(in crate::cgame::player_presenter) fn force_shells(
        &self,
        entity: &PresentedEntity,
        current_time: i32,
    ) -> Vec<(&'static str, [f32; 4], bool)> {
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
            let shader = if (hash >> 24) & 1 == 0 {
                "gfx/misc/electric"
            } else {
                "gfx/misc/fullbodyelectric2"
            };
            shells.push((shader, [1.0, 0.0, 0.0, 1.0], false));
        }
        let number = i32::from(entity.number);
        if self.look.duel_bubble && number != self.viewer_client && entity.entity_type == ET_PLAYER
        {
            // Duelists seen from outside the duel.
            shells.push((
                "gfx/misc/sightbubble",
                [100.0 / 255.0, 100.0 / 255.0, 1.0, 1.0],
                false,
            ));
        }
        if let Some(gray) = self.duel_shell_gray {
            // jaPRO cg_stylePlayer "duel shell": you and your opponent glow.
            let partner = self.viewer_style.duel_index;
            if entity.entity_type == ET_PLAYER
                && (number == partner || number == self.viewer_client)
            {
                shells.push(("powerups/forceshell", [gray, gray, gray, 1.0], false));
            }
        }
        if active & (1 << FP_PROTECT) != 0 {
            shells.push((
                "gfx/misc/forceprotect",
                [0.0, 128.0 / 255.0, 0.0, 254.0 / 255.0],
                false,
            ));
        }
        if let Some(&(until, kind)) = self.team_power.get(&entity.number) {
            let remaining = until - current_time;
            if remaining > 0 {
                if kind == 3 {
                    // Absorb: blue playerShieldDamage shell.
                    shells.push((
                        "gfx/misc/personalshield",
                        [0.0, 0.0, 1.0, 254.0 / 255.0],
                        false,
                    ));
                } else {
                    let rgb = match kind {
                        1 => [0.0, 1.0, 0.0], // heal
                        0 => [0.0, 0.0, 1.0], // regen
                        _ => [1.0, 0.0, 0.0], // drain
                    };
                    // shaderRGBA[3] = (teamPowerEffectTime - cg.time) / 8, a byte.
                    let alpha = ((remaining / 8).min(255)) as f32 / 255.0;
                    shells.push((
                        "powerups/ysalimarishell",
                        [rgb[0], rgb[1], rgb[2], alpha],
                        true,
                    ));
                }
            }
        }
        if state.field_i32("isJediMaster").unwrap_or(0) != 0 && number != self.viewer_client {
            shells.push((
                "powerups/forceshell",
                [100.0 / 255.0, 100.0 / 255.0, 1.0, 1.0],
                false,
            ));
        }
        shells
    }

    /// Material of a jaPRO ghost shader (`raceShader`/`duelShader`). Stock
    /// servers do not ship them, so fall back to the force shell they wrap.
    pub(in crate::cgame::player_presenter) fn ghost_material(
        &mut self,
        ghost: crate::japro_cg::Ghost,
    ) -> (Option<Arc<TextureData>>, DynamicModelAlphaMode) {
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
    pub(in crate::cgame::player_presenter) fn duel_shell_brightness(
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
        let viewer = [
            ps.field_f32("origin[0]")?,
            ps.field_f32("origin[1]")?,
            ps.field_f32("origin[2]")?,
        ];
        let other = origins
            .get(&partner)
            .copied()
            .filter(|_| preserve_entity != Some(partner))?;
        let distance = ((other[0] - viewer[0]).powi(2)
            + (other[1] - viewer[1]).powi(2)
            + (other[2] - viewer[2]).powi(2))
        .sqrt()
        .clamp(1.0, 1024.0);
        Some(((255.0 - distance / 4.0).max(1.0)) / 255.0)
    }

    /// `CG_DrawCosmeticOnPlayer`: hats bolted to `*head_top`, capes and held
    /// items to `*back`, from the jaPRO `c5` cosmetics mask. The MD3s themselves
    /// are submitted by the entity presenter (see [`CosmeticDraw`]).
    #[allow(clippy::too_many_arguments)]
    pub(in crate::cgame::player_presenter) fn queue_cosmetics(
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
        let mut mask = if japro_server {
            info.cosmetics
        } else {
            self.local_cosmetics
        };
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
    pub(in crate::cgame::player_presenter) fn queue_blob_shadow(
        &mut self,
        entity: &PresentedEntity,
    ) {
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
        if state.field_i32("m_iVehicleNum").unwrap_or(0) != 0
            && npc_class != crate::cgame::CLASS_VEHICLE
        {
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

    pub(in crate::cgame::player_presenter) fn push_blob_shadow_request(
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
        let reference = if normal.z.abs() < 0.9 {
            Vec3::Z
        } else {
            Vec3::X
        };
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
}

/// `customRGBA` RGB of a player/NPC entity as a 0..1 tint. A state that never
/// carried the field (all four bytes zero) is untinted; servers always send
/// alpha 255 with it, so a real black tint is still honoured.
/// The viewer's state as jaPRO's `CG_Player` reads it (`cg.snap->ps`).
pub(in crate::cgame::player_presenter) fn viewer_style_from(
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

pub(in crate::cgame::player_presenter) fn player_entity_rgb(entity: &PresentedEntity) -> [f32; 3] {
    custom_rgba_tint(std::array::from_fn(|index| {
        entity
            .state
            .field_i32(&format!("customRGBA[{index}]"))
            .unwrap_or(0)
    }))
}

pub(in crate::cgame::player_presenter) fn custom_rgba_tint(bytes: [i32; 4]) -> [f32; 3] {
    if bytes.iter().all(|&byte| byte == 0) {
        return [1.0; 3];
    }
    [0, 1, 2].map(|index| bytes[index].clamp(0, 255) as f32 / 255.0)
}

/// CG_PlayerSprites' choice of icon, in its order: connection trouble hides
/// everything, a voice command hides the talk balloon.
pub(in crate::cgame::player_presenter) fn head_sprite_shader(
    e_flags: i32,
    voice_chat_active: bool,
) -> Option<&'static str> {
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

pub(in crate::cgame::player_presenter) fn entity_is_mind_tricked(
    entity: &PresentedEntity,
    client: i32,
) -> bool {
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
pub(in crate::cgame::player_presenter) fn glm_indices_for_wgpu(mut indices: Vec<u32>) -> Vec<u32> {
    for triangle in indices.chunks_exact_mut(3) {
        triangle.swap(1, 2);
    }
    indices
}

pub(in crate::cgame::player_presenter) fn transform_model_point(
    point: [f32; 3],
    axis: [[f32; 3]; 3],
    origin: [f32; 3],
) -> [f32; 3] {
    let world = [
        origin[0] + axis[0][0] * point[0] + axis[1][0] * point[1] + axis[2][0] * point[2],
        origin[1] + axis[0][1] * point[0] + axis[1][1] * point[1] + axis[2][1] * point[2],
        origin[2] + axis[0][2] * point[0] + axis[1][2] * point[1] + axis[2][2] * point[2],
    ];
    scene::render_position(world)
}

pub(in crate::cgame::player_presenter) fn transform_model_normal(
    normal: [f32; 3],
    axis: [[f32; 3]; 3],
) -> [f32; 3] {
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
pub(in crate::cgame::player_presenter) fn player_angle_entity(
    entity: &PresentedEntity,
) -> PlayerAngleEntity {
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

pub(in crate::cgame::player_presenter) fn openjk_look_target_origin(
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
pub(in crate::cgame) fn openjk_vectoangles(value: [f32; 3]) -> [f32; 3] {
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

pub(in crate::cgame::player_presenter) fn openjk_player_entity_needs_reset(
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

pub(in crate::cgame::player_presenter) fn flatten_matrix3x4(matrix: &Matrix3x4) -> [f32; 12] {
    [
        matrix[0][0],
        matrix[0][1],
        matrix[0][2],
        matrix[0][3],
        matrix[1][0],
        matrix[1][1],
        matrix[1][2],
        matrix[1][3],
        matrix[2][0],
        matrix[2][1],
        matrix[2][2],
        matrix[2][3],
    ]
}

pub(in crate::cgame::player_presenter) fn presented_entity_velocity(
    entity: &PresentedEntity,
) -> [f32; 3] {
    [
        entity.state.field_f32("pos.trDelta[0]").unwrap_or(0.0),
        entity.state.field_f32("pos.trDelta[1]").unwrap_or(0.0),
        entity.state.field_f32("pos.trDelta[2]").unwrap_or(0.0),
    ]
}

/// Exact OpenJK BG_InKnockDownOnly semantic: the five authored knockdown
/// animations, not getups, rolls, falls, generic velocity or acceleration.
pub(in crate::cgame::player_presenter) fn presented_entity_in_knockdown(
    entity: &PresentedEntity,
) -> bool {
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

pub(in crate::cgame::player_presenter) fn impulse_weapon_is_explosive(
    weapon: i32,
    alt_fire: bool,
) -> bool {
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

pub(in crate::cgame::player_presenter) fn force_grip_target_alive(
    entity: &PresentedEntity,
) -> bool {
    (entity.entity_type == ET_PLAYER || entity.entity_type == ET_NPC)
        && entity.state.field_i32("eFlags").unwrap_or(0) & (EF_DEAD | EF_NODRAW) == 0
}

pub(in crate::cgame::player_presenter) fn force_grip_target_still_valid(
    gripper: &PresentedEntity,
    target: &PresentedEntity,
) -> bool {
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
pub(in crate::cgame::player_presenter) fn infer_force_grip_target<'a>(
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

pub(in crate::cgame::player_presenter) fn ray_default_player_box_entry(
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
pub(in crate::cgame::player_presenter) fn angles_to_axis(angles: [f32; 3]) -> [[f32; 3]; 3] {
    let (sp, cp) = angles[0].to_radians().sin_cos();
    let (sy, cy) = angles[1].to_radians().sin_cos();
    let (sr, cr) = angles[2].to_radians().sin_cos();

    let forward = [cp * cy, cp * sy, -sp];
    let right = [-sr * sp * cy + cr * sy, -sr * sp * sy - cr * cy, -sr * cp];
    let up = [cr * sp * cy + sr * sy, cr * sp * sy - sr * cy, cr * cp];
    [forward, [-right[0], -right[1], -right[2]], up]
}

pub(in crate::cgame::player_presenter) fn transform_jka_model_point(
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

pub(in crate::cgame::player_presenter) fn transform_jka_model_vector(
    vector: [f32; 3],
    axis: [[f32; 3]; 3],
) -> [f32; 3] {
    [
        axis[0][0] * vector[0] + axis[1][0] * vector[1] + axis[2][0] * vector[2],
        axis[0][1] * vector[0] + axis[1][1] * vector[1] + axis[2][1] * vector[2],
        axis[0][2] * vector[0] + axis[1][2] * vector[1] + axis[2][2] * vector[2],
    ]
}

pub(in crate::cgame::player_presenter) fn normalize_vec3(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length <= f32::EPSILON {
        [1.0, 0.0, 0.0]
    } else {
        [v[0] / length, v[1] / length, v[2] / length]
    }
}

/// CG_AddSaberBlade color selection: userinfo c1/c2 (or NPC boltToPlayer)
/// unless the NPC path uses the definition's authored per-blade color.
pub(in crate::cgame::player_presenter) fn blade_color(
    info: &crate::cgame::ClientInfo,
    client_color: i32,
    blade: &jka_assets::saber::SaberBladeDefinition,
    saber_team_colors: Option<bool>,
) -> i32 {
    let color = if info.definition_saber_colors {
        blade.color
    } else {
        client_color
    };
    let color = saber_team_colors.map_or(color, |force| {
        crate::cgame::team_saber_color(info, color, force)
    });
    crate::cgame::apply_plugin_saber_color(color, info.plugin_disable)
}

/// animTable index of a named animation (cached).
pub(in crate::cgame::player_presenter) fn animation_number(name: &str) -> Option<i32> {
    use std::sync::OnceLock;
    static TABLE: OnceLock<HashMap<&'static str, i32>> = OnceLock::new();
    TABLE
        .get_or_init(|| {
            (0..2048)
                .filter_map(|i| jka_movement::animation_name(i).map(|n| (n, i)))
                .collect()
        })
        .get(name)
        .copied()
}

/// CG_InitG2Weapons: g2WeaponInstances[giTag] = world_model[0] of the last
/// IT_WEAPON item with that tag (later bg_itemlist entries overwrite).
pub(in crate::cgame::player_presenter) fn weapon_world_model(weapon: i32) -> Option<String> {
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
pub(in crate::cgame) fn blend_for_alpha(
    mode: DynamicModelAlphaMode,
    alpha: f32,
) -> DynamicModelAlphaMode {
    if alpha >= 1.0 {
        return mode;
    }
    match mode {
        DynamicModelAlphaMode::Opaque => DynamicModelAlphaMode::Blend,
        DynamicModelAlphaMode::Mask => DynamicModelAlphaMode::MaskBlend,
        mode => mode,
    }
}

pub(in crate::cgame) fn saber_name_is_removed(name: &str) -> bool {
    name.eq_ignore_ascii_case("none") || name.eq_ignore_ascii_case("remove")
}

#[allow(clippy::too_many_arguments)]
pub(in crate::cgame::player_presenter) fn saber_blade_fx_request(
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

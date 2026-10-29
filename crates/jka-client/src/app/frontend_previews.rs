//! Frontend previews.
use crate::app::{
    local_player_alpha, offset_third_person_view, presented_openjk_player, rendering_third_person,
    scene, ui_catalog, App, Arc, AssetDetail, AssetKind, Camera, ClientInfo, Duration,
    DynamicModelSurface, EntityPresenter, FrontendPage, Ghoul2SkinningMode, Instant, JoinMode,
    PlayerFxRequest, PlayerPresenter, PlayerViewPolicyState, ProfileModelEntry, ProfileSaberEntry,
    RenderCommand, ThirdPersonViewInput, ViewBobInput, ViewLatchMode, ET_PLAYER, FRONTEND_CAMERA_EYE_HEIGHT,
    FRONTEND_FOREGROUND_MODEL, FRONTEND_FOREGROUND_MODEL_DISTANCE,
    FRONTEND_FOREGROUND_MODEL_HEAD_HEIGHT, FRONTEND_FOREGROUND_MODEL_SIDE_OFFSET,
};

/// Camera focus targets are expressed in the real isolated Studio viewport.
/// Moving the eye instead of moving the player keeps Ghoul2 body picking,
/// bolt callouts, saber FX and all renderer-side projections consistent.
#[derive(Clone, Copy)]
pub(in crate::app) struct ProfileStudioCamera {
    pub height: f32,
    pub lateral: f32,
    pub pitch: f32,
    target_height: f32,
    target_lateral: f32,
    target_pitch: f32,
    pub target_yaw: f32,
    pub target_zoom: f32,
}

impl Default for ProfileStudioCamera {
    fn default() -> Self {
        Self {
            height: -11.0, lateral: 0.0, pitch: 5.0,
            target_height: -11.0, target_lateral: 0.0, target_pitch: 5.0,
            target_yaw: 180.0, target_zoom: 1.0,
        }
    }
}

impl ProfileStudioCamera {
    /// Frame-rate-independent exponential ease with no
    /// per-frame pixel lerp or 30/60-Hz assumptions. Yaw takes the short arc.
    fn tick(&mut self, dt: f32, yaw: &mut f32, zoom: &mut f32) -> bool {
        let gain = 1.0 - (-6.5 * dt).exp();
        let mut changed = false;
        for (current, goal) in [
            (&mut self.height, self.target_height),
            (&mut self.lateral, self.target_lateral),
            (&mut self.pitch, self.target_pitch),
        ] {
            let delta = goal - *current;
            if delta.abs() > 0.002 {
                *current += delta * gain;
                changed = true;
            } else {
                *current = goal;
            }
        }
        let delta_yaw = (self.target_yaw - *yaw + 180.0).rem_euclid(360.0) - 180.0;
        if delta_yaw.abs() > 0.01 {
            *yaw = (*yaw + delta_yaw * gain).rem_euclid(360.0);
            changed = true;
        } else {
            *yaw = self.target_yaw;
        }
        let delta_zoom = self.target_zoom - *zoom;
        if delta_zoom.abs() > 0.0001 {
            *zoom = (*zoom + delta_zoom * gain).clamp(0.45, 2.5);
            changed = true;
        } else {
            *zoom = self.target_zoom;
        }
        changed
    }
}

/// One configuration for the anatomical *region*, not one for each Anny
/// slider. Actual animated Ghoul2 anchors override these height fallbacks.
/// The back-facing presets are deliberate (capes, glutes); eyes/ears obtain
/// a closer frontal or three-quarter view.
fn studio_focus_preset(region: &str) -> (f32, f32, f32, Option<usize>, f32) {
    match region {
        "cosmetic:head" | "head" => (165.0, 0.60, 37.0, Some(0), 0.0),
        "cosmetic:face" | "eyes" | "eyebrows" | "nose" | "mouth" | "chin"
        | "forehead" | "cheek" | "facial actions" => (180.0, 0.52, 35.0, Some(1), 0.0),
        "ears" => (130.0, 0.53, 34.0, Some(1), 0.0),
        "neck" => (170.0, 0.68, 31.0, Some(0), -10.0),
        "breast" => (175.0, 0.68, 19.0, Some(5), 0.0),
        "torso" => (172.0, 0.78, 15.0, Some(5), -5.0),
        "stomach" => (175.0, 0.76, 8.0, Some(5), -13.0),
        "arms" => (155.0, 0.80, 16.0, Some(3), 9.0),
        "hands" | "saber:right" => (163.0, 0.68, 5.0, Some(3), 0.0),
        "saber:left" => (197.0, 0.68, 5.0, Some(4), 0.0),
        "saber:both" => (180.0, 0.87, 8.0, Some(3), 9.0),
        "hip" | "pelvis" => (170.0, 0.78, 7.0, Some(6), 3.0),
        "buttocks" => (0.0, 0.70, 7.0, Some(6), 0.0),
        "cosmetic:back" => (0.0, 0.82, 22.0, Some(2), -14.0),
        "legs" => (168.0, 0.85, -3.0, None, 0.0),
        "feet" => (165.0, 0.65, -18.0, None, 0.0),
        _ => (180.0, 1.0, -11.0, None, 0.0),
    }
}

#[cfg(test)]
mod profile_studio_camera_tests {
    use super::{studio_focus_preset, ProfileStudioCamera};

    #[test]
    fn body_and_cosmetic_views_select_the_expected_side() {
        assert_eq!(studio_focus_preset("buttocks").0, 0.0);
        assert_eq!(studio_focus_preset("cosmetic:back").0, 0.0);
        assert!(studio_focus_preset("cosmetic:face").1 < studio_focus_preset("torso").1);
        assert_eq!(studio_focus_preset("feet").3, None);
    }

    #[test]
    fn orbit_uses_the_short_arc_and_zoom_remains_bounded() {
        let mut focus = ProfileStudioCamera::default();
        let mut yaw = 350.0;
        let mut zoom = 1.0;
        focus.target_yaw = 10.0;
        focus.target_zoom = 0.52;
        focus.tick(0.05, &mut yaw, &mut zoom);
        assert!(yaw > 350.0 || yaw < 10.0);
        assert!((0.52..1.0).contains(&zoom));
        for _ in 0..90 {
            focus.tick(0.016, &mut yaw, &mut zoom);
        }
        assert!((yaw - 10.0).abs() < 0.1);
        assert!((zoom - 0.52).abs() < 0.001);
    }
}

/// Dedicated, silent FX scheduler used only by Developer Tools. Keeping this
/// separate from CGame's weapon/map FX means previewing an .efx file can never
/// perturb a live/demo effect timeline.
pub(in crate::app) struct AssetPreviewFxState {
    pub(in crate::app) qpath: String,
    pub(in crate::app) zoom_key: i32,
    pub(in crate::app) system: crate::fx::system::FxSystem,
    pub(in crate::app) effect_id: crate::fx::system::EffectId,
    pub(in crate::app) started: Instant,
    pub(in crate::app) next_play_ms: i32,
    pub(in crate::app) cycle_ms: i32,
}


/// One repeating `fx_runner` of the menu scene (OpenJK fx_runner: play the
/// effect, then again every `delay` ms).
struct MenuFxPlayer {
    effect: crate::fx::system::EffectId,
    origin: [f32; 3],
    direction: [f32; 3],
    delay_ms: i32,
    next_ms: i32,
}

/// Menu-scene props that only the frontend needs: the map's synthesized
/// `misc_model_static` placements and its fx runners (campfire, waterfall).
/// Separate from CGame's presenters so the menu never perturbs a game session.
pub(in crate::app) struct MenuProps {
    presenter: EntityPresenter,
    fx: crate::fx::system::FxSystem,
    light_promoter: crate::fx::light_promotion::FxLightPromoter,
    pub(in crate::app) lights: Arc<Vec<crate::renderer::TransientLight>>,
    light_trace_level: Option<u8>,
    players: Vec<MenuFxPlayer>,
    started: Instant,
    /// `map_static_models` this state was built for (reload detection).
    static_source: Arc<jka_assets::bsp::StaticModels>,
}

/// JKA AngleVectors forward.
fn angles_forward(angles: [f32; 3]) -> [f32; 3] {
    let (sp, cp) = angles[0].to_radians().sin_cos();
    let (sy, cy) = angles[1].to_radians().sin_cos();
    [cp * cy, cp * sy, -sp]
}

/// Profile's animation time is in the same unquantized milliseconds as gameplay.
/// Fractional bone-frame interpolation is handled by Ghoul2Animator itself.
fn profile_preview_time_ms(elapsed: Duration) -> i32 {
    elapsed.as_millis().min(i32::MAX as u128) as i32
}

#[cfg(test)]
mod profile_animation_clock_tests {
    use super::profile_preview_time_ms;
    use std::time::Duration;

    #[test]
    fn profile_clock_preserves_sub_frame_times() {
        assert_eq!(profile_preview_time_ms(Duration::from_millis(0)), 0);
        assert_eq!(profile_preview_time_ms(Duration::from_millis(8)), 8);
        assert_eq!(profile_preview_time_ms(Duration::from_millis(16)), 16);
        assert_eq!(profile_preview_time_ms(Duration::from_millis(32)), 32);
        assert_eq!(profile_preview_time_ms(Duration::from_millis(33)), 33);
        assert_eq!(profile_preview_time_ms(Duration::from_millis(34)), 34);
    }

    #[test]
    fn profile_clock_saturates_at_ghoul2_time_limit() {
        assert_eq!(profile_preview_time_ms(Duration::from_millis(i32::MAX as u64 + 100)), i32::MAX);
    }
}

impl App {
    fn append_profile_saber_preview(&mut self, draws: &mut Vec<DynamicModelSurface>,
        fx_requests: &[PlayerFxRequest], current_time: i32) {
        // Actual saber FX bolt positions/colours, after animated
        // skeleton evaluation. Only the preview's CPU vertices are
        // affected. No world dlight pollution or map-lightgrid lookup.
        for request in fx_requests {
            if !crate::cgame::weapon_fx::profile_saber_light_allowed(request) {
                continue;
            }
            if let PlayerFxRequest::SaberBlade {origin, direction, length, color, ..} = request {
                let rgb = crate::cgame::custom_saber_rgb(*color)
                    .map(|rgb| rgb.map(|c| f32::from(c) / 255.0))
                    .unwrap_or_else(|| match *color {
                        0 => [1.0, 0.2, 0.2], 1 => [1.0, 0.5, 0.1],
                        2 => [1.0, 1.0, 0.2], 3 => [0.2, 1.0, 0.2],
                        4 => [0.2, 0.4, 1.0], 5 => [0.9, 0.2, 1.0],
                        _ => [0.45, 0.55, 0.95],
                    });
                for surface in draws.iter_mut() {
                    let vertices = Arc::make_mut(&mut surface.vertices);
                    for vertex in vertices {
                        let dx = [vertex.position[0] - origin[0],
                            vertex.position[1] - origin[1],
                            vertex.position[2] - origin[2]];
                        let projection = (dx[0]*direction[0]+dx[1]*direction[1]
                            +dx[2]*direction[2]).clamp(0.0,*length);
                        let to_light = [
                            origin[0]+direction[0]*projection-vertex.position[0],
                            origin[1]+direction[1]*projection-vertex.position[1],
                            origin[2]+direction[2]*projection-vertex.position[2],
                        ];
                        let dist_sq = to_light.iter().map(|v|v*v).sum::<f32>();
                        // Profile-only saber light. Brighten the existing
                        // diffuse/char_color_* channels rather than adding
                        // bare saber RGB (which washes out player tint).
                        // A 48u radius with linear-distance squared fade
                        // was effectively invisible beyond the saber hand.
                        const RADIUS: f32 = 112.0;
                        const INTENSITY: f32 = 1.55;
                        const MAX_VERTEX_LIGHT: f32 = 1.45;
                        const RADIUS_SQ: f32 = RADIUS * RADIUS;
                        if dist_sq >= RADIUS_SQ { continue; }
                        let distance = dist_sq.sqrt().max(0.5);
                        let facing = (vertex.normal[0]*to_light[0]
                            +vertex.normal[1]*to_light[1]
                            +vertex.normal[2]*to_light[2]) / distance;
                        // Soft local bounce lets the near side of a limb
                        // catch light even if its normal faces sideways;
                        // quadratic radius attenuation confines the glow.
                        let radial = 1.0 - dist_sq / RADIUS_SQ;
                        let strength = radial * radial
                            * (0.25 + 0.75 * facing.max(0.0)) * INTENSITY;
                        for channel in 0..3 {
                            vertex.color[channel] = (vertex.color[channel]
                                * (1.0 + rgb[channel] * strength))
                                .clamp(0.0, MAX_VERTEX_LIGHT);
                        }
                    }
                }
            }
        }
        let mut blade_draws = Vec::new();
        // Re-seeded per frame so the preview blade flickers like in game.
        let mut flicker_rng = crate::fx::template::Rng::new(
            (current_time as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15),
        );
        for request in fx_requests {
            blade_draws.extend(crate::cgame::weapon_fx::profile_saber_blade_draws_for_request(
                request,
                self.video.modern_sabers,
                &mut flicker_rng,
            ));
        }
        if !blade_draws.is_empty() {
            let view = crate::fx::draw::FxView::from_camera(&self.asset_preview_camera());
            let presenter = self
                .asset_preview_entity_presenter
                .as_mut()
                .expect("profile FX presenter initialized");
            let mut blade_surfaces =
                crate::fx::draw::tessellate(&blade_draws, &view, &mut |shader| {
                    presenter.fx_material_stages(shader)
                });
            draws.append(&mut blade_surfaces);
        }
    }

    pub(in crate::app) fn ensure_local_player_presenter(&mut self) -> Result<(), String> {
        if self.local_player_presenter.is_some() {
            return Ok(());
        }
        let mut assets =
            jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref())
                .map_err(|error| format!("SOLO PLAYER ASSET PATH ERROR: {error}"))?;
        assets.set_allow_asset_overrides(self.video.allow_asset_overrides);
        self.local_player_presenter = Some(
            PlayerPresenter::new(assets, self.video.pbr)
                .map_err(|error| format!("SOLO PLAYER ASSET ERROR: {error}"))?,
        );
        Ok(())
    }

    pub(in crate::app) fn asset_preview_camera(&self) -> Camera {
        if let Some(camera) = self.emote_gallery_camera() {
            return camera;
        }
        if self.frontend_page == FrontendPage::RenderStudio &&
            (self.profile_preview_active || self.render_studio.spatial.scene_camera.is_some()) {
            if self.render_studio.export.is_none() && self.render_studio.still.is_none() {
                if let Some(preview)=self.render_studio.cinematic_preview.as_ref() {
                    return super::render_studio_spatial::camera_from_key(preview.camera);
                }
                if let Some(key)=self.render_studio.spatial.scene_camera {
                    return super::render_studio_spatial::camera_from_key(key);
                }
            }
            return self.studio_shot_camera();
        }
        if self.profile_preview_active {
            let studio = &self.profile_studio_camera;
            let mut camera = Camera::new_with_fov(
                [0.0, studio.height, studio.lateral], 0.0, 58.0);
            camera.pitch = studio.pitch.to_radians();
            camera
        } else {
            Camera::new_with_fov([0.0, 0.0, 0.0], 0.0, 65.0)
        }
    }

    pub(in crate::app) fn studio_shot_camera(&self)->Camera {
        let key=self.render_studio.camera_rig_runtime.camera.filter(|_|self.render_studio.timeline.camera_rig.active())
            .unwrap_or_else(||self.studio_camera_key_at(self.render_studio.timeline_time()));
        super::render_studio_spatial::camera_from_key(key)
    }

    /// Called only on an explicit part/cosmetic/saber selection. Normal
    /// scrolling and mouse orbit never reissue these targets, so a user can
    /// explore manually without automatic framing fighting their input.
    pub(in crate::app) fn focus_profile_studio_camera(&mut self, region: &str) {
        let (yaw, zoom, fallback_y, anchor_index, vertical_adjust) = studio_focus_preset(region);
        let anchor = anchor_index.and_then(|idx|
            self.profile_studio_world_anchors.get(idx).copied().flatten());
        let (height, lateral) = if region.is_empty() || region == "all" || region == "global" {
            (-11.0, 0.0)
        } else {
            // Anchors are JKA-space world points, already placed and skinned.
            // Camera space uses (x,z,-y). We follow the part but do not chase
            // every step of its walking animation (which would bob the eye).
            let y = anchor.map_or(fallback_y, |point| point[2] + vertical_adjust);
            let z = anchor.map_or(0.0, |point| -point[1]);
            // Body-shape sliders float on the right, cosmetic popovers on the
            // left. Frame the character toward the *opposite* side so the
            // active editor does not obscure the area being inspected.
            let side = if region.starts_with("cosmetic:") { -6.0 } else { 4.0 };
            (y, z + side)
        };
        let camera = &mut self.profile_studio_camera;
        camera.target_height = height;
        camera.target_lateral = lateral;
        camera.target_pitch = if region.is_empty() || region == "all" || region == "global" { 5.0 } else { 0.0 };
        camera.target_yaw = yaw;
        camera.target_zoom = zoom;
        self.profile_studio_spin_velocity = 0.0;
        self.profile_preview_key = None;
    }

    pub(in crate::app) fn tick_profile_studio_camera(&mut self, dt: f32) {
        if self.profile_studio_camera.tick(dt, &mut self.profile_preview_yaw,
            &mut self.profile_preview_zoom) {
            self.profile_preview_key = None;
        }
    }

    pub(in crate::app) fn ensure_profile_catalog(&mut self) {
        if self.profile_catalog_loaded {
            return;
        }
        self.profile_catalog_loaded = true;
        self.ui_catalog.pending.profile = true;
        self.ui_catalog.request(
            &self.base,
            self.game.as_deref(),
            ui_catalog::CatalogRequest::Profile,
        );
    }

    pub(in crate::app) fn apply_profile_catalog(
        &mut self,
        result: Result<(Vec<ProfileModelEntry>, Vec<ProfileSaberEntry>), String>,
    ) {
        match result {
            Ok((models, mut sabers)) => {
                for current in [&self.network.saber1, &self.network.saber2] {
                    if !current.is_empty()
                        && !current.eq_ignore_ascii_case("none")
                        && !current.eq_ignore_ascii_case("remove")
                        && !sabers
                            .iter()
                            .any(|entry| entry.name.eq_ignore_ascii_case(current))
                    {
                        let fallback = jka_assets::saber::SaberDefinition::openjk_default(current);
                        sabers.push(ProfileSaberEntry {
                            display_name: fallback.proper_name.clone(),
                            saber_type: fallback.saber_type.clone(),
                            name: fallback.name,
                            model: fallback.model,
                            custom_skin: fallback.custom_skin,
                            num_blades: fallback.num_blades,
                            ready_anim: fallback.ready_anim,
                        });
                    }
                }
                sabers.sort_by(|a, b| {
                    a.display_name
                        .to_ascii_lowercase()
                        .cmp(&b.display_name.to_ascii_lowercase())
                        .then_with(|| {
                            a.name
                                .to_ascii_lowercase()
                                .cmp(&b.name.to_ascii_lowercase())
                        })
                });
                self.profile_models = models;
                self.profile_sabers = sabers;
                self.profile_catalog_error = None;
                // Force the preview to rebuild now that saber types are known.
                self.profile_preview_key = None;
            }
            Err(error) => {
                self.profile_catalog_error = Some(error);
            }
        }
    }

    pub(in crate::app) fn update_profile_preview(&mut self) {
        // Bone damping can evaluate historical poses. Keep the displayed actor
        // and camera on the same instant even if that first evaluation is slow.
        let previous=self.render_studio.preview_sample_time;
        if self.frontend_page==FrontendPage::RenderStudio {
            self.render_studio.preview_sample_time=Some(self.render_studio.timeline_time());
        }
        self.update_profile_preview_at_time();
        self.render_studio.preview_sample_time=previous;
    }

    fn update_profile_preview_at_time(&mut self) {
        if self.game_session.is_none() {
            let (effects,voice,separation) = self.effective_audio_mix();
            let emote_music=self.effective_emote_music_volume();
            // Studio's source monitor and character clips follow the same
            // preview-only volume as native demos and imported audio.
            let monitor_volume=if self.frontend_page==FrontendPage::RenderStudio
                && self.render_studio.export.is_none() && self.render_studio.still.is_none() {
                self.render_studio.preview_volume
            } else { 1.0 };
            if let Some(sound) = self.menu_emote_sound.as_mut() {
                sound.set_mix(effects*monitor_volume,voice*monitor_volume,separation);
                sound.set_emote_music_volume(emote_music*monitor_volume);
                sound.frame(crate::audio::Listener {
                    entity:1023,origin:[0.0;3],ahead:[1.0,0.0,0.0],up:[0.0,0.0,1.0],left:[0.0,1.0,0.0],
                }, &[], None);
            }
        } else { self.menu_emote_sound = None; }
        if !self.profile_preview_active {
            return;
        }

        self.update_studio_camera_rig();

        let force_page = self.profile_selected_section == 2;
        // Show blades is presentation-only: no network or config changes.
        let studio_saber = if self.frontend_page == FrontendPage::RenderStudio {
            self.render_studio.actor_with_sabers(self.render_studio.cinematic_preview.as_ref().map_or(0,|p|p.lane))
        } else {
            self.profile_studio_show_saber
        };
        if !force_page {
            self.profile_force_preview_pending = None;
            self.profile_force_preview_active = None;
        } else {
            if let Some((power, changed_at)) = self.profile_force_preview_pending {
                if changed_at.elapsed() >= Duration::from_millis(140) {
                    self.profile_force_preview_active = Some(power);
                    self.profile_force_preview_started = Instant::now();
                    self.profile_force_preview_pending = None;
                    self.profile_preview_key = None;
                }
            }
            // One-shot menu demonstration, then settle back to the neutral idle.
            if self.profile_force_preview_active.is_some()
                && self.profile_force_preview_started.elapsed() >= Duration::from_millis(1800)
            {
                self.profile_force_preview_active = None;
                self.profile_preview_started = Instant::now();
                self.profile_preview_key = None;
            }
        }

        let force_power = if force_page {
            self.profile_force_preview_active
        } else {
            None
        };
        let force_animation = force_power.map(|power| match power {
            0 => "BOTH_FORCEHEAL_QUICK",
            1 => "BOTH_FORCEJUMP1",
            2 => "BOTH_RUN1",
            3 => "BOTH_FORCEPUSH",
            4 => "BOTH_FORCEPULL",
            5 => "BOTH_FORCEPUSH",
            6 => "BOTH_FORCEGRIP_HOLD",
            7 => "BOTH_FORCELIGHTNING_HOLD",
            8 => "BOTH_FORCE_RAGE",
            9 => "BOTH_FORCE_PROTECT",
            10 => "BOTH_FORCE_ABSORB",
            11 => "BOTH_FORCEHEAL_QUICK",
            12 => "BOTH_FORCEPUSH",
            13 => "BOTH_FORCE_DRAIN_HOLD",
            // Seeing has no distinct stock activation character pose; saber
            // skill ranks are demonstrated in the stock saber-ready stance.
            14 => "BOTH_STAND1",
            15..=17 => "BOTH_STAND2",
            _ => "BOTH_STAND1",
        });

        let secondary_saber = !self.network.saber2.is_empty()
            && !self.network.saber2.eq_ignore_ascii_case("none")
            && !self.network.saber2.eq_ignore_ascii_case("remove");
        let primary_staff = self.profile_sabers.iter().any(|entry| {
            entry.name.eq_ignore_ascii_case(&self.network.saber1)
                && entry.saber_type.eq_ignore_ascii_case("SABER_STAFF")
        });
        // Stock JKA full-body walking clips (also used by OpenJK's character
        // preview). The model stays in place; only the Ghoul2 pose animates.
        // Match saber configuration so staff / dual hilts follow the proper
        // hands instead of forcing a single-saber or stand idle pose.
        let walk_animation = if !studio_saber {
            "BOTH_WALK1"
        } else if secondary_saber {
            "BOTH_WALK_DUAL"
        } else if primary_staff {
            "BOTH_WALK_STAFF"
        } else {
            "BOTH_WALK2"
        };
        // Force demonstrations temporarily override walking, then return to
        // the normal looping walk without changing gameplay animations.
        // Own the Studio animation name: presenter creation below requires
        // &mut App and must not overlap a borrowed field of RenderStudio.
        let studio_eval = (self.frontend_page == FrontendPage::RenderStudio)
            .then(|| self.render_studio.active_animation());
        // TaystJK bg_pmove.c::PM_GetSaberStance: a saber with readyAnim
        // replaces the stock idle stance, not active walking, attacks, emotes
        // or an explicitly selected Studio timeline clip. The profile's
        // passive showcase is an idle preview when such an override exists.
        let profile_ready_name = if studio_eval.is_none() && force_animation.is_none() && studio_saber {
            [&self.network.saber1, &self.network.saber2].into_iter().find_map(|saber_name| {
                self.profile_sabers.iter()
                    .find(|entry| entry.name.eq_ignore_ascii_case(saber_name))
                    .and_then(|entry| jka_assets::animation::AnimationSet::name(entry.ready_anim))
            })
        } else { None };
        let animation_name = if let Some((name, _, _, _)) = studio_eval.as_ref() {
            name.as_str()
        } else {
            force_animation.or(profile_ready_name).unwrap_or(walk_animation)
        };
        let with_sabers = (studio_saber || matches!(force_power, Some(15) | Some(16) | Some(17)))
            && (self.frontend_page == FrontendPage::RenderStudio || crate::emotes::active_preview().is_none());

        // The Ghoul2 evaluator already interpolates between authored GLA frames.
        // Feed it continuous millisecond time: rounding the preview clock to
        // 33-ms ticks makes *every* rendered frame repeat the same pose for up
        // to 33 ms, even when the UI and renderer can present at 120+ Hz.
        let animation_elapsed = if force_power.is_some() {
            self.profile_force_preview_started.elapsed()
        } else {
            self.profile_preview_started.elapsed()
        };
        // Preview the staged skin without mutating the live model cvar or
        // sending any userinfo until Profile actually closes.
        let preview_model = if self.frontend_page == FrontendPage::RenderStudio {
            self.render_studio.actor_model(self.render_studio.cinematic_preview.as_ref().map_or(0,|p|p.lane)).to_owned()
        } else {
            self.profile_model_staged.as_deref().map(str::to_owned)
                .unwrap_or_else(|| self.solo_client_info.model_cvar())
        };
        // Clamp only at i32::MAX to match the Ghoul2 millisecond API; never
        // quantize to animation.cfg frameLerp (Ghoul2 performs that lerp).
        let current_time = if self.frontend_page == FrontendPage::RenderStudio {
            studio_eval.as_ref().map_or(0, |eval| eval.3)
        } else {
            profile_preview_time_ms(animation_elapsed)
        };
        let studio_emote = studio_eval.as_ref().and_then(|eval| eval.1.clone());
        let studio_range = studio_eval.as_ref().and_then(|eval| eval.2);
        // Include all actor-lane animation clocks in the preview cache key.
        // Otherwise a secondary character can freeze while the lead stands idle.
        let actors_tag = if self.frontend_page == FrontendPage::RenderStudio {
            let time=self.render_studio.timeline_time();
            format!("{:?}:{:?}",self.render_studio.timeline.active_actors(time).iter().map(|clip|
                (clip.lane,clip.name.as_str(),clip.pose_time_ms(time),
                    clip.first_frame,clip.end_frame,clip.tail_frames,clip.playback,
                    self.render_studio.timeline.lane_models.get(clip.lane),
                    self.render_studio.timeline.lane_sabers.get(clip.lane),
                    self.render_studio.timeline.lane_char_colors.get(clip.lane),
                    clip.motion_at(time,self.render_studio.timeline.interpolation !=
                        super::render_studio_timeline::StudioInterpolation::Linear)))
                    .collect::<Vec<_>>(),(&self.render_studio.timeline.lane_sabers,&self.render_studio.timeline.lane_char_colors,self.render_studio.camera_rig_runtime.camera
                        .map(|k|(k.position,k.yaw,k.pitch,k.fov))))
        } else { String::new() };
        let key = (
            format!(
                "{}:{}:{}:{}:{}:{}:{}:{}:{}:{:?}:{}:{:.2}:{:.2}:{:.2}:{:.2}:{:?}",
                format!("{animation_name}:{:?}:{:?}:{studio_range:?}", studio_emote.as_ref().map(|p| p.emote.id.as_str()).or_else(|| crate::emotes::active_preview().map(|_| "profile emote")), self.profile_body_highlight),
                preview_model,
                self.network.saber1,
                self.network.saber2,
                with_sabers,
                self.network.color1,
                self.network.color2,
                self.network.sb_rgb1,
                self.network.sb_rgb2,
                self.network.char_color,
                self.network.cosmetics,
                self.profile_studio_head_look[0],
                self.profile_studio_head_look[1],
                self.video.jiggle_breast_size,
                self.video.jiggle_glute_size,
                (self.video.body_shape_params(), jka_assets::body_shape::anny::fingerprint(self.video.body_shape_params(), self.video.body_details)),
            ) + actors_tag.as_str(),
            (self.profile_preview_yaw * 10.0).round() as i32,
            (self.profile_preview_zoom * 1000.0).round() as i32,
            current_time,
        );
        if self.profile_preview_key.as_ref() == Some(&key) {
            return;
        }

        // Only Render Studio applies authored character motion. Normal Profile
        // previews remain unchanged and continue to use the global model turntable.
        let studio_motion = (self.frontend_page == FrontendPage::RenderStudio && self.render_studio.cinematic_preview.is_none())
            .then(|| self.render_studio.lane_motion(0,self.render_studio.timeline_time()))
            .flatten().unwrap_or_default();
        let lead_blend = (self.frontend_page == FrontendPage::RenderStudio)
            .then(|| self.render_studio.pose_blend_request(0)).flatten();
        let yaw = (self.profile_preview_yaw + studio_motion.yaw).to_radians();
        let (s, c) = yaw.sin_cos();
        let axis = [[c, s, 0.0], [-s, c, 0.0], [0.0, 0.0, 1.0]];
        let zoom = self.profile_preview_zoom.clamp(0.45, 2.5);
        let mut info = self.solo_client_info.clone();
        if let Some(model) = (self.frontend_page == FrontendPage::RenderStudio)
            .then_some(preview_model.as_str())
            .or(self.profile_model_staged.as_deref()) {
            let staged = ClientInfo::solo_model(model);
            info.model_name = staged.model_name;
            info.skin_name = staged.skin_name;
        }
        info.female = self.network.sex.as_bytes().first()
            .is_some_and(|byte| byte.eq_ignore_ascii_case(&b'f'));
        info.saber_name = self.network.saber1.clone();
        info.saber2_name = self.network.saber2.clone();
        // Preview what a jaPRO / JA+ server would show, RGB included.
        info.saber_color = crate::cgame::resolve_saber_color(
            i32::from(self.network.color1),
            self.network.sb_rgb1 as i32,
            true,
        );
        info.saber2_color = crate::cgame::resolve_saber_color(
            i32::from(self.network.color2),
            self.network.sb_rgb2 as i32,
            true,
        );
        info.plugin_disable = self.network.plugin_disable;
        let preview_char_color=if self.frontend_page == FrontendPage::RenderStudio {
            self.render_studio.actor_char_color(self.render_studio.cinematic_preview.as_ref().map_or(0,|p|p.lane),self.network.char_color)
        } else {self.network.char_color};
        if self.frontend_page == FrontendPage::RenderStudio {
            self.render_studio.apply_actor_sabers(self.render_studio.cinematic_preview.as_ref().map_or(0,|p|p.lane),&mut info);
        }

        let result = (|| -> Result<Vec<DynamicModelSurface>, String> {
            self.ensure_asset_preview_player_presenter()?;
            let cosmetics = self.network.cosmetics;
            if with_sabers || cosmetics != 0 || (self.frontend_page == FrontendPage::RenderStudio &&
                (0..self.render_studio.timeline.lanes).any(|lane|self.render_studio.actor_with_sabers(lane))) {
                self.ensure_asset_preview_entity_presenter()?;
            }
            let origin = [88.0 * zoom + studio_motion.position[0],
                studio_motion.position[1], -22.0 + studio_motion.position[2]];
            let (mut draws, fx_requests) = {
                let presenter = self
                    .asset_preview_player_presenter
                    .as_mut()
                    .expect("profile Ghoul2 presenter initialized");
                presenter.set_skinning_mode(Ghoul2SkinningMode::Cpu);
                presenter.set_early_frustum_cull(false);
                presenter.set_lod_bias(self.video.ghoul2_lod_bias);
                presenter.set_lod_scale(self.video.lod_scale);
                presenter.set_ghoul2_anim_smooth(if self.frontend_page == FrontendPage::RenderStudio { 0.0 } else { self.video.ghoul2_anim_smooth });
                // Profile and Render Studio have their own presenter, but use
                // the same jiggle enablement, sex filter and tuning as gameplay.
                // Their animation clocks never advance the live player's state.
                presenter.set_hair_physics_mode(self.video.hair_physics);
                presenter.set_jiggle_config(
                    self.video.client_physics && self.video.jiggle_physics != 0,
                    self.video.jiggle_physics,
                    self.video.client_physics_hz,
                    self.video.client_physics_max_substeps,
                    crate::cgame::jiggle::JiggleTuning {
                        solver: crate::cgame::jiggle::JiggleSolver::from_u8(self.video.jiggle_solver),
                        overall_strength: self.video.jiggle_strength,
                        breast_strength: self.video.jiggle_breast_strength,
                        glute_strength: self.video.jiggle_glute_strength,
                        stomach_strength: self.video.jiggle_stomach_strength,
                        stiffness_scale: self.video.jiggle_stiffness,
                        damping_scale: self.video.jiggle_damping,
                        glute_jiggle_bottom_z: self.video.jiggle_glute_jiggle_bottom_z,
                        glute_jiggle_top_z: self.video.jiggle_glute_jiggle_top_z,
                        jp_stiffness: self.video.jiggle_jp_stiffness,
                        jp_drag: self.video.jiggle_jp_drag,
                        jp_air_drag: self.video.jiggle_jp_air_drag,
                        jp_stretch: self.video.jiggle_jp_stretch,
                        jp_soften: self.video.jiggle_jp_soften,
                        jp_gravity: self.video.jiggle_jp_gravity,
                        locomotion: self.video.jiggle_locomotion,
                        impulse_cap: self.video.jiggle_impulse_cap,
                        swirl: self.video.jiggle_swirl,
                        breast_stiffness: self.video.jiggle_breast_stiffness,
                        breast_damping: self.video.jiggle_breast_damping,
                        glute_stiffness: self.video.jiggle_glute_stiffness,
                        glute_damping: self.video.jiggle_glute_damping,
                        stomach_damping: self.video.jiggle_stomach_damping,
                        stomach_stiffness: self.video.jiggle_stomach_stiffness,
                        breast_size: self.video.jiggle_breast_size,
                        glute_size: self.video.jiggle_glute_size,
                    },
                    crate::cgame::jiggle::JiggleRemeshTuning {
                        body_shape: self.video.body_shape_params(),
                        body_details: self.video.body_details,
                        breast_bottom_z: self.video.jiggle_breast_remesh_bottom_z,
                        breast_top_z: self.video.jiggle_breast_remesh_top_z,
                        glute_bottom_z: self.video.jiggle_glute_remesh_bottom_z,
                        glute_top_z: self.video.jiggle_glute_remesh_top_z,
                        breast_size: self.video.jiggle_breast_size,
                        glute_size: self.video.jiggle_glute_size,
                    },
                );
                // No stale requests from a prior Profile frame should leak into
                // the current isolated saber preview.
                let _ = presenter.drain_fx_requests();
                presenter.set_profile_body_highlight(self.profile_body_highlight.as_deref());
                presenter.studio_pose_blend = lead_blend.clone();
                let (draws, anchors) = presenter.present_profile_player_preview(
                    &info,
                    origin,
                    axis,
                    current_time,
                    animation_name,
                    with_sabers,
                    preview_char_color.map(|channel| f32::from(channel) / 255.0),
                    cosmetics,
                    self.profile_studio_head_look,
                    true,
                    if self.frontend_page == FrontendPage::RenderStudio {
                        studio_emote.clone()
                    } else {
                        crate::emotes::active_preview()
                    },
                    studio_range,
                )?;
                let requests = presenter.drain_fx_requests();
                self.profile_studio_world_anchors = anchors;
                (draws, requests)
            };

            // Hats and capes the player has picked, live as they click.
            let worn = self
                .asset_preview_player_presenter
                .as_mut()
                .expect("profile Ghoul2 presenter initialized")
                .drain_cosmetic_draws();
            if !worn.is_empty() {
                // Emote props can produce attachments even with cosmetics and
                // sabers disabled. Initialize after seeing the actual draw list.
                self.ensure_asset_preview_entity_presenter()?;
                self.asset_preview_entity_presenter.as_mut().ok_or("profile attachment presenter unavailable")?.set_material_eye([0.;3]);
                let mut hats = self
                    .asset_preview_entity_presenter
                    .as_mut()
                    .ok_or("profile attachment presenter unavailable")?
                    .present_cosmetics(worn);
                crate::cgame::player_presenter::apply_profile_studio_light(&mut hats, true);
                draws.append(&mut hats);
            }

            let (emote_fx, _) = self
                .asset_preview_player_presenter
                .as_mut()
                .expect("profile Ghoul2 presenter initialized")
                .drain_emote_fx();
            if !emote_fx.is_empty() {
                self.ensure_asset_preview_entity_presenter()?;
                let view = crate::fx::draw::FxView::from_camera(&self.asset_preview_camera());
                let presenter = self
                    .asset_preview_entity_presenter
                    .as_mut()
                    .ok_or("profile effect presenter unavailable")?;
                draws.extend(crate::fx::draw::tessellate(&emote_fx, &view, &mut |shader| {
                    presenter.fx_material_stages(shader)
                }));
            }

            if with_sabers {
                self.append_profile_saber_preview(&mut draws,&fx_requests,current_time);
            }
            if self.frontend_page == FrontendPage::RenderStudio {
                let t=self.render_studio.timeline_time();
                let has_primary=self.render_studio.cinematic_preview.is_some() || self.render_studio.timeline.clip_at_lane(t,0).is_some();
                if !has_primary {draws.clear();}
                // Each occupied layer is a separate Ghoul2 actor. The existing
                // presenter is reused sequentially: each call returns owned
                // skinned surfaces, then the surfaces are composited in one pass.
                // Separate Y offsets make two actors visible by default; motion
                // keys still control each actor's local XYZ/yaw independently.
                let actors:Vec<_>=self.render_studio.timeline.active_actors(t).into_iter()
                    .filter(|c|c.lane>0 && self.render_studio.cinematic_preview.is_none()).map(|c|{
                        let motion=self.render_studio.lane_motion(c.lane,t).unwrap_or_else(||c.motion_at(t,
                            self.render_studio.timeline.interpolation!=super::render_studio_timeline::StudioInterpolation::Linear));
                        let model=self.render_studio.actor_model(c.lane).to_owned();
                        (c.lane,c.name.clone(),c.pose_time_ms(t),c.preview(t),
                            (c.first_frame,c.end_frame),motion,model,self.render_studio.pose_blend_request(c.lane))
                    }).collect();
                for (lane,name,time_ms,emote,frame_range,motion,model,blend) in actors {
                    // Keep each character independent. A missing skin, unsupported
                    // external pose, or bad emote must not discard the successful
                    // draw surfaces of the other characters in this frame.
                    let actor_result = (|| -> Result<Vec<DynamicModelSurface>, String> {
                        let mut actor_info=info.clone();
                        // Each track starts from the profile, not the lead actor's overrides.
                        self.studio_profile_sabers().apply(&mut actor_info);
                        self.render_studio.apply_actor_sabers(lane,&mut actor_info);
                        let staged=ClientInfo::solo_model(&model);
                        actor_info.model_name=staged.model_name;
                        actor_info.skin_name=staged.skin_name;
                        let yaw=(self.profile_preview_yaw+motion.yaw).to_radians();
                        let (sn,cs)=yaw.sin_cos();
                        let actor_axis=[[cs,sn,0.0],[-sn,cs,0.0],[0.0,0.0,1.0]];
                        // The renderer's preview stage uses Y-up and Z-sideways.
                        // Stagger characters horizontally, not vertically.
                        let actor_origin=[88.0*zoom+motion.position[0],
                            motion.position[1],-22.0+lane as f32*26.0+motion.position[2]];
                        let presenter=self.asset_preview_player_presenter.as_mut()
                            .ok_or("Studio player presenter unavailable")?;
                        presenter.studio_pose_blend=blend.clone();
                        let (mut actor_draws,_) = presenter.present_profile_player_preview(
                            &actor_info,actor_origin,actor_axis,time_ms,&name,
                            self.render_studio.actor_with_sabers(lane),self.render_studio.actor_char_color(lane,self.network.char_color)
                                .map(|channel|f32::from(channel)/255.0),
                            0,self.profile_studio_head_look,true,emote,Some(frame_range))?;
                        // Additional actors use the same attachment presenter as
                        // the primary; no network or game session entities needed.
                        let saber_fx=presenter.drain_fx_requests();
                        let worn=presenter.drain_cosmetic_draws();
                        let _=presenter.drain_emote_fx();
                        if !worn.is_empty() {
                            self.ensure_asset_preview_entity_presenter()?;
                            let mut attachments=self.asset_preview_entity_presenter.as_mut()
                                .ok_or("Studio attachment presenter unavailable")?.present_cosmetics(worn);
                            actor_draws.append(&mut attachments);
                        }
                        if self.render_studio.actor_with_sabers(lane) {
                            self.append_profile_saber_preview(&mut actor_draws,&saber_fx,time_ms);
                        }
                        Ok(actor_draws)
                    })();
                    match actor_result {
                        Ok(mut actor_draws) => draws.append(&mut actor_draws),
                        Err(error) => {
                            // Errors partway through presentation may leave staged
                            // props/FX in the shared presenter. Do not leak them
                            // into the next actor's draw or the following frame.
                            if let Some(presenter) = self.asset_preview_player_presenter.as_mut() {
                                let _=presenter.drain_fx_requests();
                                let _=presenter.drain_cosmetic_draws();
                                let _=presenter.drain_emote_fx();
                            }
                            let message=format!(
                                "RENDER STUDIO ACTOR {} ({model}, {name}): {error}",lane+1);
                            if self.console_status != message {
                                self.console_status=message.clone();
                                self.push_console_line(format!("^1{message}"));
                            }
                            self.render_studio.status=message;
                        }
                    }
                }
            }
            Ok(draws)
        })();

        match result {
            Ok(draws) => {
                self.profile_dynamic_models = Arc::new(draws);
                self.profile_preview_key = Some(key);
            }
            Err(error) => {
                self.profile_dynamic_models = Arc::new(Vec::new());
                self.profile_preview_key = Some(key);
                let message = format!("PROFILE PREVIEW: {error}");
                if self.console_status != message {
                    self.console_status = message.clone();
                    self.push_console_line(format!("^1{message}"));
                }
            }
        }
    }

    pub(in crate::app) fn ensure_asset_preview_entity_presenter(&mut self) -> Result<(), String> {
        if let Some(presenter) = self.asset_preview_entity_presenter.as_mut() {
            presenter.set_md3_geometry_cache(self.video.md3_geometry_cache);
            return Ok(());
        }
        let mut assets =
            jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref())
                .map_err(|error| format!("ASSET VIEWER VFS ERROR: {error}"))?;
        assets.set_allow_asset_overrides(self.video.allow_asset_overrides);
        self.asset_preview_entity_presenter = Some(
            EntityPresenter::new(assets, self.video.pbr)
                .map_err(|error| format!("ASSET VIEWER MD3 ERROR: {error}"))?,
        );
        self.asset_preview_entity_presenter.as_mut().unwrap().set_md3_geometry_cache(self.video.md3_geometry_cache);
        Ok(())
    }

    pub(in crate::app) fn ensure_asset_preview_player_presenter(&mut self) -> Result<(), String> {
        if self.asset_preview_player_presenter.is_some() {
            return Ok(());
        }
        let mut assets =
            jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref())
                .map_err(|error| format!("ASSET VIEWER VFS ERROR: {error}"))?;
        assets.set_allow_asset_overrides(self.video.allow_asset_overrides);
        self.asset_preview_player_presenter = Some(
            PlayerPresenter::new(assets, self.video.pbr)
                .map_err(|error| format!("ASSET VIEWER GLM ERROR: {error}"))?,
        );
        Ok(())
    }

    pub(in crate::app) fn clear_asset_preview_runtime(&mut self) {
        self.solo_dynamic_models = Arc::new(Vec::new());
        self.asset_preview_model_key = None;
        self.asset_preview_shader_key = None;
        self.asset_preview_fx = None;
    }

    pub(in crate::app) fn set_asset_preview_viewport(
        &mut self,
        rect: egui::Rect,
        pixels_per_point: f32,
    ) {
        let scale = pixels_per_point.max(0.01);
        let viewport = [
            (rect.min.x * scale).round().max(0.0) as u32,
            (rect.min.y * scale).round().max(0.0) as u32,
            (rect.width() * scale).round().max(1.0) as u32,
            (rect.height() * scale).round().max(1.0) as u32,
        ];
        if self.asset_preview_viewport_key != Some(viewport) {
            self.asset_preview_viewport_key = Some(viewport);
            self.render_command(RenderCommand::SetAssetPreviewViewport(Some(viewport)));
        }
    }

    pub(in crate::app) fn update_asset_preview_content(&mut self) {
        if self.frontend_page != FrontendPage::AssetViewer {
            return;
        }
        let Some(detail) = self
            .asset_viewer_detail
            .as_ref()
            .and_then(|detail| detail.as_ref().ok())
            .cloned()
        else {
            self.clear_asset_preview_runtime();
            return;
        };

        match detail.kind {
            AssetKind::Md3 | AssetKind::Glm => self.update_asset_model_preview(&detail),
            AssetKind::Shader => self.update_asset_shader_preview(&detail),
            AssetKind::Efx => self.update_asset_efx_preview(&detail),
        }
    }

    pub(in crate::app) fn update_asset_model_preview(&mut self, detail: &AssetDetail) {
        let key = (
            detail.qpath.clone(),
            (self.asset_viewer_model_yaw * 10.0).round() as i32,
            (self.asset_viewer_model_pitch * 10.0).round() as i32,
            (self.asset_viewer_model_zoom * 1000.0).round() as i32,
        );
        if self.asset_preview_model_key.as_ref() == Some(&key) {
            return;
        }

        let center = detail.model_center.unwrap_or([0.0; 3]);
        let radius = detail.model_radius.unwrap_or(32.0).max(1.0);
        let distance = (radius * 2.45 * self.asset_viewer_model_zoom).clamp(24.0, 7500.0);
        let yaw = self.asset_viewer_model_yaw.to_radians();
        let pitch = self.asset_viewer_model_pitch.to_radians();
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        // Match the existing yaw convention, then pitch around the model's
        // horizontal axis. This keeps drag rotation centered on the asset rather
        // than pitching the viewer camera away from the preview target.
        let axis = [
            [cy * cp, sy * cp, -sp],
            [-sy, cy, 0.0],
            [cy * sp, sy * sp, cp],
        ];
        let rotated_center = [
            axis[0][0] * center[0] + axis[1][0] * center[1] + axis[2][0] * center[2],
            axis[0][1] * center[0] + axis[1][1] * center[1] + axis[2][1] * center[2],
            axis[0][2] * center[0] + axis[1][2] * center[1] + axis[2][2] * center[2],
        ];
        let target = [distance, 0.0, 0.0];
        let origin = [
            target[0] - rotated_center[0],
            target[1] - rotated_center[1],
            target[2] - rotated_center[2],
        ];
        let qpath = detail.qpath.clone();
        let kind = detail.kind;

        let result = match kind {
            AssetKind::Md3 => self.ensure_asset_preview_entity_presenter().and_then(|_| {
                self.asset_preview_entity_presenter
                    .as_mut()
                    .expect("asset MD3 presenter initialized")
                    .present_static_md3(0, &qpath, origin, axis, [1.0; 4], 0)
            }),
            AssetKind::Glm => self.ensure_asset_preview_player_presenter().and_then(|_| {
                let presenter = self
                    .asset_preview_player_presenter
                    .as_mut()
                    .expect("asset GLM presenter initialized");
                presenter.set_skinning_mode(self.video.ghoul2_skinning);
                presenter.set_early_frustum_cull(false);
                presenter.set_lod_bias(self.video.ghoul2_lod_bias);
                presenter.set_lod_scale(self.video.lod_scale);
                presenter.present_static_glm_preview(0, &qpath, origin, axis, [1.0; 4], 0)
            }),
            AssetKind::Efx | AssetKind::Shader => unreachable!(),
        };
        match result {
            Ok(draws) => {
                self.solo_dynamic_models = Arc::new(draws);
                self.asset_preview_model_key = Some(key);
            }
            Err(error) => {
                self.solo_dynamic_models = Arc::new(Vec::new());
                self.asset_preview_model_key = Some(key);
                let message = format!("ASSET VIEWER PREVIEW: {qpath}: {error}");
                if self.console_status != message {
                    self.console_status = message.clone();
                    self.push_console_line(format!("^1{message}"));
                }
            }
        }
    }

    pub(in crate::app) fn update_asset_shader_preview(&mut self, detail: &AssetDetail) {
        let Some(shader_name) = self
            .asset_viewer_shader_name
            .clone()
            .or_else(|| detail.shader_names.first().cloned())
        else {
            self.solo_dynamic_models = Arc::new(Vec::new());
            self.asset_preview_shader_key = None;
            return;
        };
        let zoom_key = (self.asset_viewer_model_zoom * 1000.0).round() as i32;
        let key = (detail.qpath.clone(), shader_name.clone(), zoom_key);
        if self.asset_preview_shader_key.as_ref() == Some(&key) {
            return;
        }
        if let Err(error) = self.ensure_asset_preview_entity_presenter() {
            self.solo_dynamic_models = Arc::new(Vec::new());
            self.asset_preview_shader_key = Some(key);
            self.push_console_line(format!("^1ASSET VIEWER SHADER PREVIEW: {error}"));
            return;
        }

        // A material browser is most useful on a neutral, camera-facing card.
        // Use the existing FX material resolver so blend modes and multi-stage
        // JKA shaders are represented by the same textures/pipelines as gameplay.
        let distance = (112.0 * self.asset_viewer_model_zoom).clamp(32.0, 3000.0);
        let half = 48.0;
        // Put a neutral checker behind the authored material. Besides making
        // alpha obvious, this gives filter/modulate stages something sensible
        // to composite against instead of the viewer's black clear color.
        let mut draws = Vec::with_capacity(65);
        let cells = 8_u32;
        let cell = half * 2.0 / cells as f32;
        for y in 0..cells {
            for z in 0..cells {
                let y0 = -half + y as f32 * cell;
                let y1 = y0 + cell;
                let z0 = -half + z as f32 * cell;
                let z1 = z0 + cell;
                let shade = if (y + z) & 1 == 0 { 96 } else { 144 };
                draws.push(crate::fx::system::FxDraw::Quad {
                    positions: [
                        [distance + 0.5, y0, z1],
                        [distance + 0.5, y1, z1],
                        [distance + 0.5, y1, z0],
                        [distance + 0.5, y0, z0],
                    ],
                    uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    rgba: [shade, shade, shade, 255],
                    shader: "$whiteimage".to_owned(),
                });
            }
        }
        draws.push(crate::fx::system::FxDraw::Quad {
            positions: [
                [distance, -half, half],
                [distance, half, half],
                [distance, half, -half],
                [distance, -half, -half],
            ],
            uvs: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            rgba: [255, 255, 255, 255],
            shader: shader_name.clone(),
        });
        let view = crate::fx::draw::FxView::from_camera(&self.asset_preview_camera());
        let presenter = self
            .asset_preview_entity_presenter
            .as_mut()
            .expect("asset shader presenter initialized");
        let surfaces = crate::fx::draw::tessellate(&draws, &view, &mut |shader| {
            presenter.fx_material_stages(shader)
        });
        self.solo_dynamic_models = Arc::new(surfaces);
        self.asset_preview_shader_key = Some(key);
    }

    pub(in crate::app) fn ensure_asset_preview_fx(
        &mut self,
        detail: &AssetDetail,
    ) -> Result<(), String> {
        let zoom_key = (self.asset_viewer_model_zoom * 1000.0).round() as i32;
        if self
            .asset_preview_fx
            .as_ref()
            .is_some_and(|state| state.qpath == detail.qpath && state.zoom_key == zoom_key)
        {
            return Ok(());
        }

        let mut assets =
            jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref())
                .map_err(|error| format!("ASSET VIEWER EFX VFS ERROR: {error}"))?;
        assets.set_allow_asset_overrides(self.video.allow_asset_overrides);
        let mut system = crate::fx::system::FxSystem::new();
        let effect_id = system.register(&detail.qpath, &mut |qpath| {
            assets
                .read(qpath, 32 * 1024 * 1024)
                .ok()
                .flatten()
                .map(|asset| asset.bytes)
        });
        if effect_id == 0 {
            return Err(format!("could not register {}", detail.qpath));
        }
        system.reset_time(0);
        self.asset_preview_fx = Some(AssetPreviewFxState {
            qpath: detail.qpath.clone(),
            zoom_key,
            system,
            effect_id,
            started: Instant::now(),
            next_play_ms: 0,
            cycle_ms: detail.preview_cycle_ms.unwrap_or(1500).clamp(250, 15_000),
        });
        Ok(())
    }

    pub(in crate::app) fn update_asset_efx_preview(&mut self, detail: &AssetDetail) {
        if let Err(error) = self.ensure_asset_preview_entity_presenter() {
            self.solo_dynamic_models = Arc::new(Vec::new());
            self.push_console_line(format!("^1ASSET VIEWER EFX PREVIEW: {error}"));
            return;
        }
        if let Err(error) = self.ensure_asset_preview_fx(detail) {
            self.solo_dynamic_models = Arc::new(Vec::new());
            let message = format!("ASSET VIEWER EFX PREVIEW: {error}");
            if self.console_status != message {
                self.console_status = message.clone();
                self.push_console_line(format!("^1{message}"));
            }
            return;
        }

        let distance = (112.0 * self.asset_viewer_model_zoom).clamp(24.0, 8000.0);
        let frame = {
            let state = self
                .asset_preview_fx
                .as_mut()
                .expect("asset EFX state initialized");
            let now_ms = state.started.elapsed().as_millis().min(i32::MAX as u128) as i32;
            let now_ms = now_ms.max(1);
            state.system.adjust_time(now_ms);
            if now_ms >= state.next_play_ms {
                state.system.play_effect_dir(
                    state.effect_id,
                    [distance, 0.0, 0.0],
                    [1.0, 0.0, 0.0],
                );
                state.next_play_ms = now_ms.saturating_add(state.cycle_ms);
            }
            state.system.frame()
        };

        // The viewer is intentionally silent; Sound primitives remain metadata.
        // Visual primitives are run by the real FX scheduler and material path.
        let view = crate::fx::draw::FxView::from_camera(&self.asset_preview_camera());
        let presenter = self
            .asset_preview_entity_presenter
            .as_mut()
            .expect("asset EFX presenter initialized");
        let surfaces = crate::fx::draw::tessellate(&frame.draws, &view, &mut |shader| {
            presenter.fx_material_stages(shader)
        });
        self.solo_dynamic_models = Arc::new(surfaces);
    }

    /// Only capture a reusable menu frame after the actual player model has
    /// settled AND its held idle animation has been submitted for multiple frames.
    /// WorldUploaded alone is insufficient because Ghoul2 models stream later.
    fn advance_frontend_scene_readiness(&mut self, model_ready: bool, current_time: i32) {
        let now = Instant::now();
        let Some(cinematic) = self.frontend_cinematic.as_mut() else { return; };
        if cinematic.world_ready_at.is_none() || cinematic.fade_started.is_some() {
            return;
        }
        if !model_ready {
            cinematic.model_visible_since = None;
            cinematic.pose_hold_ms = None;
            cinematic.pose_locked_at = None;
            return;
        }
        let first_visible = *cinematic.model_visible_since.get_or_insert(now);
        // Let initial Ghoul2 anim transitions settle, without recording a
        // transient bind/landing pose or the fallback model from a cold load.
        if now.saturating_duration_since(first_visible) < Duration::from_millis(850) {
            return;
        }
        if cinematic.pose_locked_at.is_none() {
            cinematic.pose_hold_ms = Some(current_time);
            cinematic.pose_locked_at = Some(now);
            return;
        }
        // The held pose must have passed through the app snapshot and actual
        // render thread several times before the scene-only GPU readback.
        if now.saturating_duration_since(cinematic.pose_locked_at.unwrap())
            < Duration::from_millis(200)
        {
            return;
        }
        cinematic.fade_started = Some(now);
        // Pages with a renderer-owned viewport (e.g. `uipage renderstudio` at
        // launch) replace the menu scene; capturing then would cache that page
        // as the next launch's menu backdrop.
        let scene_page = !matches!(self.frontend_page, FrontendPage::RenderStudio | FrontendPage::Profile
            | FrontendPage::AssetViewer | FrontendPage::MapViewer | FrontendPage::Screenshots);
        if let Some(path) = self.frontend_scene_cache_path.as_ref().filter(|path| scene_page && !path.exists()) {
            self.render_command(RenderCommand::CaptureFrontendScene {
                path: path.clone(),
                neutral_gamma: super::frontend_scene_cache::shader_gamma_reusable(&self.video),
            });
        }
        self.egui_repaint_requested = true;
    }

    /// Static props and FX of the menu scene, as dynamic model surfaces. Empty
    /// for maps without a menu look. Built lazily and rebuilt when the prepared
    /// map's placements change.
    fn menu_scene_props(&mut self) -> Vec<DynamicModelSurface> {
        let runners: Vec<scene::MapFxRunner> = self
            .prepared_map_cache
            .as_ref()
            .map(|cache| cache.map.fx_runners.clone())
            .unwrap_or_default();
        let statics = Arc::clone(&self.map_static_models);
        if runners.is_empty() && statics.placements.is_empty() {
            self.menu_props = None;
            return Vec::new();
        }
        let stale = self
            .menu_props
            .as_ref()
            .is_none_or(|props| !Arc::ptr_eq(&props.static_source, &statics));
        if stale {
            let mut assets =
                match jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref()) {
                    Ok(assets) => assets,
                    Err(error) => {
                        eprintln!("MENU PROPS VFS ERROR: {error}");
                        return Vec::new();
                    }
                };
            assets.set_allow_asset_overrides(self.video.allow_asset_overrides);
            let mut presenter = match EntityPresenter::new(assets, self.video.pbr) {
                Ok(presenter) => presenter,
                Err(error) => {
                    eprintln!("MENU PROPS MD3 ERROR: {error}");
                    return Vec::new();
                }
            };
            presenter.set_md3_geometry_cache(self.video.md3_geometry_cache);
            presenter.set_static_md3_instances(self.video.static_md3_instances);
            presenter.set_static_md3_bvh(self.video.static_md3_bvh);
            presenter.set_static_md3_pvs_fallback(self.video.static_md3_pvs_fallback);
            presenter.set_static_models(Arc::clone(&statics));

            let mut fx_assets =
                match jka_assets::pk3::AssetSearchPath::open_game(&self.base, self.game.as_deref()) {
                    Ok(assets) => assets,
                    Err(error) => {
                        eprintln!("MENU PROPS FX VFS ERROR: {error}");
                        return Vec::new();
                    }
                };
            fx_assets.set_allow_asset_overrides(self.video.allow_asset_overrides);
            let mut fx = crate::fx::system::FxSystem::new();
            fx.set_light_promotion_catalog(Arc::clone(&self.fx_light_promotion_catalog));
            let mut light_promoter = crate::fx::light_promotion::FxLightPromoter::default();
            light_promoter.set_catalog(Arc::clone(&self.fx_light_promotion_catalog));
            let mut players = Vec::new();
            for runner in &runners {
                let effect = fx.register(&runner.effect, &mut |qpath| {
                    fx_assets
                        .read(qpath, 32 * 1024 * 1024)
                        .ok()
                        .flatten()
                        .map(|asset| asset.bytes)
                });
                if effect == 0 {
                    eprintln!("MENU PROPS: could not register effect {}", runner.effect);
                    continue;
                }
                players.push(MenuFxPlayer {
                    effect,
                    origin: runner.origin,
                    direction: angles_forward(runner.angles),
                    delay_ms: runner.delay_ms.max(50),
                    next_ms: 0,
                });
            }
            fx.reset_time(0);
            self.menu_props = Some(MenuProps {
                presenter,
                fx,
                light_promoter,
                lights: Arc::new(Vec::new()),
                light_trace_level: None,
                players,
                started: Instant::now(),
                static_source: statics,
            });
        }
        let Some(props) = self.menu_props.as_mut() else { return Vec::new() };
        let mut surfaces = props.presenter.present_static_models(None);
        if !props.players.is_empty() {
            let now_ms = (props.started.elapsed().as_millis().min(i32::MAX as u128) as i32).max(1);
            props.fx.adjust_time(now_ms);
            for player in &mut props.players {
                if now_ms >= player.next_ms {
                    props.fx.play_effect_dir(player.effect, player.origin, player.direction);
                    player.next_ms = now_ms.saturating_add(player.delay_ms);
                }
            }
            let mut frame = props.fx.frame();
            let promoted = props.light_promoter.promote(
                &frame.auto_light_candidates,
                &mut |shader| props.presenter.fx_material_stages(shader),
                now_ms,
                self.video.promote_fx_lights,
            );
            if props.light_trace_level != Some(self.video.promote_fx_lights) {
                rverbose!(2, "[MENU FX LIGHTS] promotion={} | candidates={} | authored={} | promoted={}",
                    self.video.promote_fx_lights, frame.auto_light_candidates.len(),
                    frame.lights.len(), promoted.len());
                props.light_trace_level = Some(self.video.promote_fx_lights);
            }
            frame.lights.extend(promoted);
            props.lights = Arc::new(frame.lights.iter()
                .filter_map(crate::renderer::TransientLight::from_fx).collect());
            let view = crate::fx::draw::FxView::from_camera(&self.camera);
            let presenter = &mut props.presenter;
            surfaces.extend(crate::fx::draw::tessellate(&frame.draws, &view, &mut |shader| {
                presenter.fx_material_stages(shader)
            }));
        }
        surfaces
    }

    pub(in crate::app) fn update_frontend_foreground_player(&mut self) {
        let Some(cinematic) = self.frontend_cinematic else {
            self.solo_dynamic_models = Arc::new(Vec::new());
            return;
        };
        if let Err(error) = self.ensure_local_player_presenter() {
            if self.console_status != error {
                self.console_status = error.clone();
                self.push_console_line(format!("^1{error}"));
            }
            self.solo_dynamic_models = Arc::new(Vec::new());
            return;
        }

        let stage = scene::menu_stage(super::constants::FRONTEND_BACKGROUND_MAP);
        if stage.is_some_and(|stage| !stage.player) {
            // An empty stage: nothing to wait for before the handoff.
            self.solo_dynamic_models = Arc::new(self.menu_scene_props());
            self.advance_frontend_scene_readiness(true, 0);
            return;
        }
        let Some(idle_anim) = jka_assets::animation::animation_index(
            stage.map_or("BOTH_STAND1", |stage| stage.player_anim),
        )
        .map(|index| index as i32) else {
            self.solo_dynamic_models = Arc::new(Vec::new());
            return;
        };

        let forward = self.camera.forward();
        let right = glam::Vec3::new(-forward.z, 0.0, forward.x).normalize_or_zero();
        let model_origin = self.camera.position
            + forward * FRONTEND_FOREGROUND_MODEL_DISTANCE
            + right * FRONTEND_FOREGROUND_MODEL_SIDE_OFFSET;
        let model_feet_render = [
            model_origin.x,
            model_origin.y - FRONTEND_FOREGROUND_MODEL_HEAD_HEIGHT,
            model_origin.z,
        ];
        let mut model_feet_jka = scene::jka_position(model_feet_render);
        let camera_jka = scene::jka_position(self.camera.position.to_array());
        let to_camera_x = camera_jka[0] - model_feet_jka[0];
        let to_camera_y = camera_jka[1] - model_feet_jka[1];
        let mut face_camera_yaw = to_camera_y.atan2(to_camera_x).to_degrees();
        if let Some(stage) = stage {
            // Authored staging: the player sits where the scene put them, not
            // beside the camera.
            model_feet_jka = stage.player_feet;
            face_camera_yaw = stage.player_yaw;
        }

        let mut entity_view = jka_movement::PlayerEntityView::default();
        entity_view.number = 0;
        entity_view.entity_type = ET_PLAYER;
        entity_view.client_num = 0;
        // Camera/world rendering uses Y-up render coordinates, while the shared
        // OpenJK player presentation path consumes native JKA Z-up coordinates.
        entity_view.origin = model_feet_jka;
        entity_view.angles = [0.0, face_camera_yaw, 0.0];
        entity_view.legs_anim = idle_anim;
        entity_view.torso_anim = idle_anim;
        entity_view.health = 100;
        entity_view.view_height = FRONTEND_CAMERA_EYE_HEIGHT as i32;
        entity_view.ground_entity_num = jka_movement::ENTITY_WORLD;

        let Some(entity) = presented_openjk_player(entity_view) else {
            self.solo_dynamic_models = Arc::new(Vec::new());
            return;
        };

        // Share the menu camera's stationary->animated timeline: the player
        // must not advance while the cached frame is fading into live 3D.
        let current_time = cinematic.pose_hold_ms.map_or_else(
            || cinematic.anim_started.elapsed().as_millis().min(i32::MAX as u128) as i32,
            |held| {
                let resumed = cinematic.fade_started.map_or(0, |start| {
                    Instant::now().saturating_duration_since(
                        self.frontend_visual_handoff_start(start) + Duration::from_secs_f32(
                            crate::app::egui_menu::FRONTEND_SCENE_FADE_IN_SECS,
                        ),
                    ).as_millis().min(i32::MAX as u128) as i32
                });
                held.saturating_add(resumed)
            },
        );
        let presenter = self
            .local_player_presenter
            .as_mut()
            .expect("local player presenter initialized");
        presenter.set_skinning_mode(self.video.ghoul2_skinning);
        presenter.set_early_frustum_cull(self.video.ghoul2_early_cull);
        presenter.set_lod_bias(self.video.ghoul2_lod_bias);
        presenter.set_ghoul2_anim_smooth(self.video.ghoul2_anim_smooth);
        presenter.set_lod_scale(self.video.lod_scale);
        let info = ClientInfo::solo_model(FRONTEND_FOREGROUND_MODEL);
        let result = presenter.present_player_entity(
            &entity,
            &info,
            current_time,
            1.0,
            Some(scene::jka_position(self.camera.position.to_array())),
            false,
            true,
            false,
            None,
            false, // Menu mannequin: no authoritative movement mode.
        );
        let settled = presenter.player_model_settled(entity.number);
        match result {
            Ok(draws) => {
                let model_ready = settled && !draws.is_empty();
                let mut draws = draws;
                draws.extend(self.menu_scene_props());
                self.solo_dynamic_models = Arc::new(draws);
                self.advance_frontend_scene_readiness(model_ready, current_time);
            }
            Err(error) => {
                self.solo_dynamic_models = Arc::new(Vec::new());
                self.advance_frontend_scene_readiness(false, current_time);
                if self.console_status != error {
                    self.console_status = error.clone();
                    self.push_console_line(format!("^1FRONTEND PLAYER PRESENTATION: {error}"));
                }
            }
        }
    }

    pub(in crate::app) fn update_solo_player_view_and_presentation(&mut self) {
        self.render_view_latch = ViewLatchMode::Disabled;
        self.camera.clear_first_person_bob();
        if self
            .game_session
            .as_ref()
            .is_some_and(|session| session.local)
        {
            // `/map` and `/devmap` are presented exclusively by the shared
            // snapshot/CGame path. Keeping the legacy direct presenter active
            // here would double-submit the player and, more importantly, let
            // local-only FX behavior drift from joined servers again.
            self.solo_dynamic_models = Arc::new(Vec::new());
            return;
        }
        let mut first_person_camera = self.camera;
        first_person_camera.clear_first_person_bob();
        let Some((
            mode,
            player_view,
            entity_view,
            view_angles,
            presentation_origin,
            presentation_time,
        )) = self.local_server.as_ref().map(|player| {
            player.camera_subframe(&mut first_person_camera);
            let player_view = player.view();
            let view_angles = player.subframe_view_angles();
            (
                player.mode,
                player_view,
                player.entity_view(),
                view_angles,
                player.presentation_origin(),
                player.presentation_time(),
            )
        })
        else {
            if self.front_end {
                if self.frontend_page != FrontendPage::AssetViewer {
                    self.update_frontend_foreground_player();
                }
            } else {
                self.solo_dynamic_models = Arc::new(Vec::new());
            }
            return;
        };
        let policy = PlayerViewPolicyState::from_entity_view(entity_view, player_view.legs_timer);
        let render_third_person = mode == JoinMode::Player
            && rendering_third_person(self.third_person, self.first_person_lightsaber, policy);
        let first_person_saber = mode == JoinMode::Player
            && !render_third_person
            && self.first_person_lightsaber
            && matches!(policy.weapon, 2 | 3);
        let latch_input_available =
            mode == JoinMode::Player && policy.health > 0 && policy.vehicle_num == 0;

        if render_third_person {
            if let Some(world) = self.map_collision.as_mut() {
                let view = offset_third_person_view(
                    world,
                    self.third_person,
                    &mut self.third_person_camera,
                    ThirdPersonViewInput {
                        origin: presentation_origin,
                        view_angles,
                        velocity: entity_view.velocity,
                        view_height: entity_view.view_height,
                        health: entity_view.health,
                        dead_yaw: entity_view.dead_yaw as f32,
                        client_num: entity_view.client_num,
                        time: f64::from(presentation_time),
                        teleported: false,
                    },
                );
                self.camera.position = glam::Vec3::from_array(scene::render_position(view.origin));
                self.camera.yaw = view.angles[1].to_radians();
                self.camera.pitch = -view.angles[0].to_radians();
                if latch_input_available {
                    if let Some(latch) = view.late_latch {
                        self.render_view_latch = ViewLatchMode::ThirdPerson(latch);
                    }
                }
            } else {
                self.camera = first_person_camera;
                if latch_input_available {
                    self.render_view_latch = ViewLatchMode::Direct;
                }
            }
        } else {
            self.camera = first_person_camera;
            self.camera.apply_first_person_bob(self.view_bob, ViewBobInput {
                angles: view_angles,
                velocity: player_view.velocity,
                bob_cycle: player_view.bob_cycle,
                pm_flags: player_view.pm_flags,
                pm_type: player_view.pm_type,
                health: entity_view.health,
            });
            if latch_input_available {
                self.render_view_latch = ViewLatchMode::Direct;
            }
        }

        // As in OpenJK CG_Player, keep the local player animation/Ghoul2 state
        // running even in first person. Geometry submission is the only thing
        // suppressed there (RF_THIRD_PERSON in OpenJK).
        if mode == JoinMode::Player && entity_view.entity_type == ET_PLAYER {
            if let Err(error) = self.ensure_local_player_presenter() {
                if self.console_status != error {
                    self.console_status = error.clone();
                    self.push_console_line(format!("^1{error}"));
                }
                self.solo_dynamic_models = Arc::new(Vec::new());
                return;
            }
            let mut render_entity_view = entity_view;
            render_entity_view.origin = presentation_origin;
            render_entity_view.angles = view_angles;
            if let Some(entity) = presented_openjk_player(render_entity_view) {
                let player_presentation_time = presentation_time;
                let ghoul2_view = self.current_ghoul2_view();
                let presenter = self
                    .local_player_presenter
                    .as_mut()
                    .expect("local player presenter initialized");
                presenter.set_skinning_mode(self.video.ghoul2_skinning);
                presenter.set_early_frustum_cull(self.video.ghoul2_early_cull);
                presenter.set_lod_bias(self.video.ghoul2_lod_bias);
                presenter.set_ghoul2_anim_smooth(self.video.ghoul2_anim_smooth);
                presenter.set_lod_scale(self.video.lod_scale);
                let result = presenter.present_player_entity(
                    &entity,
                    &self.solo_client_info,
                    player_presentation_time,
                    local_player_alpha(self.third_person.alpha),
                    None,
                    false,
                    render_third_person || first_person_saber,
                    first_person_saber,
                    ghoul2_view,
                    player_view.pm_type == 0, // PM_NORMAL; noclip is 3.
                );
                match result {
                    Ok(draws) => self.solo_dynamic_models = Arc::new(draws),
                    Err(error) => {
                        self.solo_dynamic_models = Arc::new(Vec::new());
                        if self.console_status != error {
                            self.console_status = error.clone();
                            self.push_console_line(format!("^1SOLO PLAYER PRESENTATION: {error}"));
                        }
                    }
                }
            } else {
                self.solo_dynamic_models = Arc::new(Vec::new());
            }
        } else {
            self.solo_dynamic_models = Arc::new(Vec::new());
        }
    }
}

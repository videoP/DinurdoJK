//! Previews.
use crate::cgame::player_presenter::{
    animation_index, apply_profile_studio_light, blade_color, cosmetic_draws_for_mask,
    model_bolt_matrix_timed, normalize_vec3, record_pose_eval, saber_blade_fx_request,
    saber_hand_bolt_matrix_timed,
    saber_name_is_removed, sibling_default_skin_qpath, transform_jka_model_point,
    transform_jka_model_vector, Arc, DynamicModelSurface, Ghoul2Animator, Instant, PlayerPresenter,
    BONE_ANIM_OVERRIDE_FREEZE, BONE_ANIM_OVERRIDE_LOOP,
};

/// External emote display names may not exist in animation.cfg. The emote supplies its own pose,
/// while the shared presenter still needs a stock animation entry for fallback metadata.
fn preview_animation_index(name: &str, has_external_emote: bool) -> Result<i32, String> {
    animation_index(name)
        .or_else(|| if has_external_emote { animation_index("BOTH_STAND1") } else { None })
        .map(|index| index as i32)
        .ok_or_else(|| format!("missing animation table entry {name}"))
}
// Shared diagnostic for Profile and any Render Studio path that calls
// present_profile_player_preview. Disabled by default: no render-time I/O.
// Set DINURDO_SABER_DIAG=1 before launching for automatic captures in Studio,
// or toggle "Saber debug" in Profile. Never changes Ghoul2 transforms.
static PREVIEW_SABER_DEBUG: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static PREVIEW_SABER_DEBUG_FROM_ENV: std::sync::OnceLock<bool> =
    std::sync::OnceLock::new();
static PREVIEW_SABER_DEBUG_STATE: std::sync::OnceLock<
    std::sync::Mutex<(String, Option<std::time::Instant>)>,
> = std::sync::OnceLock::new();

pub(crate) fn set_saber_preview_debug(enabled: bool) {
    PREVIEW_SABER_DEBUG.store(enabled, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn saber_preview_debug_enabled() -> bool {
    PREVIEW_SABER_DEBUG.load(std::sync::atomic::Ordering::Relaxed)
        || *PREVIEW_SABER_DEBUG_FROM_ENV.get_or_init(|| {
            std::env::var("DINURDO_SABER_DIAG").ok().as_deref() == Some("1")
        })
}

pub(crate) fn saber_preview_debug_snapshot() -> String {
    PREVIEW_SABER_DEBUG_STATE.get_or_init(Default::default)
        .lock().map(|state| state.0.clone())
        .unwrap_or_else(|_| "Saber debug mutex poisoned".to_owned())
}

fn publish_saber_preview_debug(report: String) {
    let now = std::time::Instant::now();
    let should_log = if let Ok(mut state) = PREVIEW_SABER_DEBUG_STATE
        .get_or_init(Default::default).lock() {
        state.0 = report.clone();
        if state.1.is_none_or(|last| now.duration_since(last) >= std::time::Duration::from_secs(1)) {
            state.1 = Some(now);
            true
        } else { false }
    } else { false };
    if should_log {
        // One sample per second, only when enabled. CWD-relative so the user
        // can find the file beside their launch script/executable.
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true)
            .open("saber-attachment-debug.log") {
            let _ = writeln!(file, "{}\n---", report);
        }
    }
}

fn preview_saber_axes(
    matrix: &jka_assets::ghoul2::Matrix3x4,
    axis: [[f32; 3]; 3],
) -> [[f32; 3]; 3] {
    std::array::from_fn(|column| {
        normalize_vec3(transform_jka_model_vector(
            [matrix[0][column], matrix[1][column], matrix[2][column]], axis))
    })
}

pub(super) struct BodyPick {
    pub(super) model: Arc<super::assets::types::PlayerModelAsset>,
    pose: Vec<jka_assets::ghoul2::Matrix3x4>,
    axis: [[f32;3];3],
    origin: [f32;3],
}

impl PlayerPresenter {
    /// Intersect the animated visible body mesh. Sabers, cosmetics and hidden
    /// skin surfaces do not participate. Only clicks perform CPU intersection.
    pub(crate) fn pick_profile_body(&self, info: &crate::cgame::ClientInfo,
        ray_origin: [f32;3], ray_direction: [f32;3]) -> Option<&'static str> {
        let pick = self.profile_body_pick.as_ref()?;
        let key = super::player_model_key(info);
        // A model selection/loading transition must not pick the previous body.
        let super::assets::loading::ModelLookup::Ready(current) = self.lookup_model(&key) else {return None;};
        if !Arc::ptr_eq(&current, &pick.model) || !pick.model.body_shape_available {return None;}
        let local_origin = std::array::from_fn(|i| (0..3).map(|k|
            (ray_origin[k]-pick.origin[k])*pick.axis[i][k]).sum::<f32>());
        let local_direction = std::array::from_fn(|i| (0..3).map(|k|
            ray_direction[k]*pick.axis[i][k]).sum::<f32>());
        let ray_origin = glam::Vec3::from_array(local_origin);
        let direction = glam::Vec3::from_array(local_direction);
        let mut nearest = f32::INFINITY;
        let mut bind_point = None;
        let lod = pick.model.glm.lods.first()?;
        for surface in &lod.surfaces {
            if !pick.model.surfaces.iter().any(|s| s.surface_index==surface.surface_index && s.default_visible) {continue;}
            let skinned = jka_assets::ghoul2::skin_glm_surface(surface, &pick.pose).ok()?;
            for face in &surface.triangles {
                let ids = face.map(|i|i as usize);
                let [a,b,c] = ids.map(|i|glam::Vec3::from_array(skinned.vertices[i].position));
                let ab=b-a; let ac=c-a; let h=direction.cross(ac); let det=ab.dot(h);
                if det.abs()<1e-7 {continue;}
                let s=ray_origin-a;let u=s.dot(h)/det;
                if !(0.0..=1.0).contains(&u) {continue;}
                let q=s.cross(ab);let v=direction.dot(q)/det;
                if v<0.0 || u+v>1.0 {continue;}
                let distance=ac.dot(q)/det;
                if distance<=0.0 || distance>=nearest {continue;}
                nearest=distance;
                let weights=[1.0-u-v,u,v];
                bind_point=Some(std::array::from_fn(|k|(0..3).map(|j|surface.vertices[ids[j]].position[k]*weights[j]).sum::<f32>()));
            }
        }
        jka_assets::body_shape::anny::region(&pick.model.glm,&pick.model.gla,
            pick.model.source_native_pivots.as_deref().map(|p|p.as_slice()),bind_point?)
    }
    pub(crate) fn profile_body_shape_available(&self, info: &crate::cgame::ClientInfo) -> bool {
        let mut local_info = info.clone();
        local_info.local_body = true;
        let key = super::player_model_key(&local_info);
        matches!(self.lookup_model(&key), super::assets::loading::ModelLookup::Ready(model) if model.body_shape_available)
    }
    /// Render one externally supplied pose using the model's native mesh rig.
    /// This is the input seam for future animation decoders; sampling, timing
    /// and clip playback remain the caller's responsibility.
    #[allow(dead_code)]
    pub fn present_player_pose_preview(
        &mut self,
        info: &crate::cgame::ClientInfo,
        source_skeleton: &[jka_assets::ghoul2::GlaBone],
        input: jka_assets::skeleton::PoseInput<'_>,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let model = self.load_model(info)?;
        let plan = jka_assets::skeleton::cached_retargeter(source_skeleton, &model.gla.skeleton)?;
        let pose = model.bind_mesh_pose(plan.retarget(input)?)?;
        self.render_glm_surfaces(
            0, &info.model_qpath(), &model.glm, &model.gla, &model.surfaces,
            self.jiggle_profile_for_sex(model.jiggle.as_deref(), model.female),
            &pose, 0, axis, origin, [1.0; 4], None, true,
        )
    }

    pub(in crate::cgame::player_presenter) fn load_emote_gla(
        &mut self, qpath: &str,
    ) -> Result<Arc<jka_assets::ghoul2::GlaAnimation>, String> {
        let cell = {
            let mut cache = self.load_shared.glas.lock()
                .map_err(|_| "Ghoul2 animation cache poisoned".to_owned())?;
            Arc::clone(cache.entry(qpath.to_ascii_lowercase()).or_default())
        };
        cell.get_or_init(|| {
            if let Some(bytes) = crate::emotes::read_asset(qpath, jka_emotes::MAX_MEMBER_BYTES)? {
                jka_assets::ghoul2::parse_gla(&bytes).map(Arc::new)
            } else { self.load_glm_animation(qpath) }
        }).clone()
    }

    /// `TurretClientRun` from TaystJK. This is deliberately separate from
    /// `present_static_glm`: the pickup remains a static registration while
    /// deployed (bolt1) ET_GENERAL entities animate on their own clock.
    #[allow(clippy::too_many_arguments)]
    pub fn present_turret_glm(
        &mut self,
        entity_num: u16,
        model_qpath: &str,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        rgba: [f32; 4],
        started_at: i32,
        current_time: i32,
        hinge_angles: [f32; 3],
        muzzle_flash: bool,
    ) -> Result<(Vec<DynamicModelSurface>, Option<([f32; 3], [f32; 3])>), String> {
        let model = self.load_static_glm(model_qpath, None, model_qpath)?;
        let mut animator = Ghoul2Animator::new(&model.animation_gla);
        // Exactly CG_TurretClientRun's model_root clip, not a generic GLM idle.
        animator.set_bone_anim(&model.animation_gla, "model_root", 0, 11,
            BONE_ANIM_OVERRIDE_FREEZE, 0.8, started_at, None, 0)?;
        for bone in ["bone_gback", "bone_barrel"] {
            animator.set_bone_angles_postmult(&model.animation_gla, bone,
                [0.0; 3], jka_movement::G2_POSITIVE_Y,
                jka_movement::G2_POSITIVE_Z, jka_movement::G2_POSITIVE_X)?;
        }
        animator.set_bone_angles_replace(&model.animation_gla, "bone_hinge",
            hinge_angles, jka_movement::G2_NEGATIVE_Y,
            jka_movement::G2_NEGATIVE_Z, jka_movement::G2_NEGATIVE_X)?;
        let pose_started = Instant::now();
        let pose = model.bind_animation_pose(
            animator.evaluate_pose_openjk_root(&model.animation_gla, current_time)?)?;
        record_pose_eval(&mut self.perf, pose_started);
        // CG_TurretClientRun takes the origin and NEGATIVE_X of *flash02.
        let muzzle = if muzzle_flash {
            model_bolt_matrix_timed(&mut self.perf, &model.glm, &model.gla, &pose, "*flash02")?
                .map(|bolt| {
                    let local = [bolt[0][3], bolt[1][3], bolt[2][3]];
                    let dir = [-bolt[0][0], -bolt[1][0], -bolt[2][0]];
                    (transform_jka_model_point(local, axis, origin),
                     transform_jka_model_vector(dir, axis))
                })
        } else { None };
        let draws = self.render_glm_surfaces(entity_num, model_qpath,
            &model.glm, &model.gla, &model.surfaces, None, &pose,
            0, axis, origin, rgba, None, true)?;
        Ok((draws, muzzle))
    }

    /// Submit a Ghoul2 model in its default pose as one refEntity: `axis` may
    /// carry a non-uniform scale (CG_Item's 1.5x weapons), `rgba` is the
    /// RF_RGB_TINT/RF_FORCE_ENT_ALPHA shaderRGBA and `custom_shader` replaces
    /// every surface's material like refEntity.customShader.
    pub fn present_static_glm(
        &mut self,
        entity_num: u16,
        model_qpath: &str,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        rgba: [f32; 4],
        custom_shader: Option<&str>,
        current_time: i32,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let mut model = self.load_static_glm(model_qpath, None, model_qpath)?;
        // TaystJK/OpenJK CG_General initializes the Ghoul2 model first, then
        // checks G2API_SkinlessModel and, when needed, applies the sibling
        // `model_default.skin`. Our no-skin registration yields no drawable
        // surfaces for that same class of GLM, so perform the identical
        // fallback before submitting the entity. A missing default skin is
        // non-fatal in OpenJK (R_RegisterSkin returns handle 0), so retain the
        // original registration if this optional lookup fails.
        if model.surfaces.is_empty() {
            if let Some(default_skin) = sibling_default_skin_qpath(model_qpath) {
                if let Ok(skinned) =
                    self.load_static_glm(model_qpath, Some(&default_skin), model_qpath)
                {
                    model = skinned;
                }
            }
        }
        let pose_started = Instant::now();
        let pose =
            model.bind_animation_pose(Ghoul2Animator::new(&model.animation_gla).evaluate_pose_openjk_root(&model.animation_gla, current_time)?)?;
        record_pose_eval(&mut self.perf, pose_started);
        let custom = custom_shader.map(|shader| self.custom_shader_material(shader));
        let draws = self.render_glm_surfaces(
            entity_num,
            model_qpath,
            &model.glm,
            &model.gla,
            &model.surfaces,
            None,
            &pose,
            0,
            axis,
            origin,
            rgba,
            custom,
            true,
        )?;
        Ok(draws)
    }

    /// Profile preview path for a real JKA player selection. Unlike the generic
    /// Asset Viewer, this resolves the exact model[/skin] through ClientInfo, so
    /// multipart skins and `*off` surface mappings behave exactly like CG_Player.
    // Asset-preview entry point that is not wired into the viewer yet.
    #[allow(dead_code)]
    pub fn present_static_player_preview(
        &mut self,
        info: &crate::cgame::ClientInfo,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        current_time: i32,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let model = self.load_model(info)?;
        let pose_started = Instant::now();
        let pose =
            model.bind_animation_pose(Ghoul2Animator::new(&model.animation_gla).evaluate_pose_openjk_root(&model.animation_gla, current_time)?)?;
        record_pose_eval(&mut self.perf, pose_started);
        self.render_glm_surfaces(
            0,
            &info.model_qpath(),
            &model.glm,
            &model.gla,
            &model.surfaces,
            self.jiggle_profile_for_sex(model.jiggle.as_deref(), model.female),
            &pose,
            0,
            axis,
            origin,
            [1.0; 4],
            None,
            true,
        )
    }

    /// Authored animation span for the editor (absolute first / exclusive last).
    pub fn profile_preview_animation_span(&self, name: &str) -> Option<(i32, i32, f32)> {
        let index = animation_index(name)? as i32;
        let animation = self.animations.get(index)?;
        if animation.frame_lerp == 0 || animation.num_frames == 0 { return None; }
        let first = i32::from(animation.first_frame);
        let last = first + i32::from(animation.num_frames);
        Some((first.min(last), first.max(last), 1000.0 / f32::from(animation.frame_lerp).abs()))
    }

    /// Modern Profile viewport: stock humanoid walk-in-place animation plus
    /// the selected saber hilts bolted to the same hand tags used by CG_Player.
    /// This is deliberately a thin presentation path over the normal assets;
    /// model/skin/saber semantics remain OpenJK-compatible userinfo values.
    /// Shared skeletal evaluation for rendering and deterministic camera samples.
    /// Historical camera samples never advance preview physics or pose history.
    pub(super) fn profile_preview_pose(
        &mut self, model: &Arc<super::assets::types::PlayerModelAsset>,
        origin: [f32;3], axis: [[f32;3];3], current_time: i32,
        animation_name: &str, head_look: [f32;2],
        emote_preview: Option<&crate::emotes::Preview>, frame_range: Option<(i32,i32)>,
        presentation: bool,
    ) -> Result<Vec<jka_assets::ghoul2::Matrix3x4>, String> {
        // Cache converted GLAs through the existing asset loader.
        let emote_gla = if let Some(active) = emote_preview.as_ref() {
            let clip = self.load_emote_gla(&active.emote.gla)?;
            crate::emotes::pose_plan(&clip, &model.gla)?;
            if active.emote.end_frame as usize > active.emote.pose.as_ref().map_or(clip.num_frames, |p| p.frames) {
                return Err(format!("emote {} ends at frame {}, GLA has {} frames",
                    active.emote.id, active.emote.end_frame, clip.num_frames));
            }
            Some(clip)
        } else {
            None
        };
        let playback_gla = emote_gla.as_ref().unwrap_or(&model.animation_gla);
        let animation_number = preview_animation_index(animation_name, emote_preview.is_some())?;
        let animation = *self
            .animations
            .get(animation_number)
            .ok_or_else(|| format!("missing humanoid animation {animation_name}"))?;
        if animation.frame_lerp == 0 {
            return Err(format!(
                "humanoid animation {animation_name} has zero frameLerp"
            ));
        }
        let anim_speed = 50.0 / f32::from(animation.frame_lerp);
        let (first_frame, last_frame) = if anim_speed < 0.0 {
            (
                i32::from(animation.first_frame) + i32::from(animation.num_frames),
                i32::from(animation.first_frame),
            )
        } else {
            (
                i32::from(animation.first_frame),
                i32::from(animation.first_frame) + i32::from(animation.num_frames),
            )
        };
        let flags = if animation.loop_frames != -1 {
            BONE_ANIM_OVERRIDE_LOOP
        } else {
            BONE_ANIM_OVERRIDE_FREEZE
        };
        let baked_pose = emote_preview.as_ref().and_then(|active|
            active.emote.pose.as_ref().map(|clip|crate::emotes::sample_body(clip,active)));
        let (first_frame, last_frame, flags, anim_speed, pose_time) =
            if let Some(active) = emote_preview.as_ref() {
                let spec = &active.emote;
                (spec.first_frame, spec.end_frame,
                    if spec.looping { BONE_ANIM_OVERRIDE_LOOP } else { BONE_ANIM_OVERRIDE_FREEZE },
                    spec.fps / 20.0, active.elapsed_ms)
            } else {
                (first_frame, last_frame, flags, anim_speed, current_time.max(0))
            };
        let (first_frame, last_frame, pose_time) = if let Some((start, end)) = frame_range {
            // UI values are absolute GLA frames. Keep the requested subclip
            // inside the authored animation; a malformed range must not read
            // beyond a GLA or yield a zero-frame Ghoul2 animation.
            let lo = first_frame.min(last_frame);
            let hi = first_frame.max(last_frame);
            let start = start.clamp(lo, hi.saturating_sub(1));
            let end = end.clamp(start + 1, hi);
            let offset_ms = emote_preview.as_ref().map_or(0, |active| {
                ((start - active.emote.first_frame).max(0) as f32 * 1000.0
                    / active.emote.fps.max(1.0)).round() as i32
            });
            (start, end, pose_time.saturating_sub(offset_ms))
        } else {
            (first_frame, last_frame, pose_time)
        };
        // Compact bundles use the GLA only as a skeleton. Their frame ranges
        // belong to animation.pose, so never register them on the one-frame GLA.
        let mut animation_pose = match baked_pose {
            Some(pose) => pose,
            None => {
                let mut animator = Ghoul2Animator::new(playback_gla);
                // CG_Player uses model_root for legs and lower_lumbar for torso. For a
                // full-body menu stand both use the same authored animation; Motion is
                // updated too, matching the same-animation branch in CG_PlayerAnimation.
                for bone in ["model_root", "lower_lumbar", "Motion"] {
                    if Ghoul2Animator::bone_index(playback_gla, bone).is_some() {
                        animator.set_bone_anim(
                            playback_gla,
                            bone,
                            first_frame,
                            last_frame,
                            flags,
                            anim_speed,
                            0,
                            None,
                            0,
                        )?;
                    }
                }
                // The same cranium/cervical postmult conventions as OpenJK
                // BG_G2PlayerAngles (POSITIVE_X, NEGATIVE_Y, NEGATIVE_Z). Apply before
                // pose evaluation, so hats, sabers and the entire model stay coherent.
                let pitch = (head_look[1] * 15.0).clamp(-15.0, 15.0);
                let yaw = (head_look[0] * 24.0).clamp(-24.0, 24.0);
                for (bone, weight) in [("cervical", 0.34), ("cranium", 0.66)] {
                    if Ghoul2Animator::bone_index(playback_gla, bone).is_some() {
                        animator.set_bone_angles_postmult(
                            playback_gla, bone, [pitch * weight, yaw * weight, 0.0],
                            jka_movement::G2_POSITIVE_X,
                            jka_movement::G2_NEGATIVE_Y,
                            jka_movement::G2_NEGATIVE_Z,
                        )?;
                    }
                }
                animator.evaluate_pose_openjk_root(playback_gla, pose_time)?
            }
        };
        if presentation && emote_preview.is_none() && self.ghoul2_anim_smooth > 0.0 && self.ghoul2_anim_smooth < 1.0 {
            const MAX_HISTORY_GAP_MS: i32 = 250;
            if let Some((old_model, old_animation, old_time, previous)) =
                self.profile_bone_smooth_history.as_ref()
            {
                if old_model == &model.key
                    && old_animation == animation_name
                    && current_time > *old_time
                    && current_time.saturating_sub(*old_time) <= MAX_HISTORY_GAP_MS
                    && previous.len() == animation_pose.len()
                {
                    animation_pose = super::smooth_ghoul2_pose(
                        &model.animation_gla, previous, &animation_pose,
                        self.ghoul2_anim_smooth,
                    );
                }
            }
            self.profile_bone_smooth_history = Some((
                model.key.clone(), animation_name.to_owned(), current_time,
                animation_pose.clone(),
            ));
        } else if presentation {
            self.profile_bone_smooth_history = None;
        }
        let mut pose = if emote_preview.is_some() {
            model.bind_external_pose(playback_gla, animation_pose)?
        } else {
            model.bind_animation_pose(animation_pose)?
        };
        if presentation && model.source_jiggles_enabled {
            if let Some(bindings) = model.source_jiggles.as_deref() {
                self.preview_source_jiggles
                    .entry(model.key.clone())
                    .or_default()
                    .update(bindings, model.gla.skeleton.len(), &mut pose,
                            axis, origin, current_time);
            }
        }
        if let Some(active)=emote_preview.as_ref(){model.correct_emote_ground(active,&mut pose)?;}
        Ok(pose)
    }

    pub fn present_profile_player_preview(
        &mut self,
        info: &crate::cgame::ClientInfo,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        current_time: i32,
        animation_name: &str,
        with_sabers: bool,
        skin_tint: [f32; 3],
        cosmetics: u32,
        // Normalized cursor offset in the Studio stage, used for the player's
        // authored cervical/cranium Ghoul2 look bones.
        head_look: [f32; 2],
        studio_cinematic: bool,
        // A menu emote is a presentation-only override, never an entityState
        // animation: the playing preview, or a scrubbed one (emote_gallery).
        emote_preview: Option<crate::emotes::Preview>,
        frame_range: Option<(i32, i32)>,
    ) -> Result<(Vec<DynamicModelSurface>, [Option<[f32; 3]>; 7]), String> {
        // This presentation route doesn't call present_snapshot_players, which
        // normally advances the stage clock. The async jiggle worker needs a
        // monotonically advancing preview timeline to integrate the walk pose.
        self.stage_time_ms = current_time.max(0);
        let mut local_info = info.clone();
        local_info.local_body = true;
        let info = &local_info;
        let model = self.load_model(info)?;
        let pose_started = Instant::now();
        let pose = self.profile_preview_pose(&model, origin, axis, current_time,
            animation_name, head_look, emote_preview.as_ref(), frame_range, true)?;
        let animation_number = preview_animation_index(animation_name, emote_preview.is_some())?;
        if let Some(active)=emote_preview.as_ref(){
            self.cosmetic_draws.extend(Self::emote_prop_draws(active,&model,&pose,axis,origin,0));
            let mut fx=Self::emote_effect_frame(active,&model,&pose,axis,origin,0);
            self.cosmetic_draws.append(&mut fx.meshes);
            self.emote_fx.draws.append(&mut fx.draws);
            self.emote_fx.lights.append(&mut fx.lights);
        }
        let mut idle=Self::idle_effect_frame(&model,&pose,axis,origin,0,current_time);
        self.cosmetic_draws.append(&mut idle.meshes);
        self.emote_fx.draws.append(&mut idle.draws);
        self.emote_fx.lights.append(&mut idle.lights);
        // The UI anchors use the *same animated Ghoul2 pose* as skinning,
        // including cursor-driven cranium/cervical edits and model yaw.
        // No fixed screen percentages or arbitrary player-space guesses.
        let mut anchors = [None; 7];
        for (index, tag) in [(0, "*head_top"), (2, "*back"),
            (3, "*r_hand"), (4, "*l_hand")] {
            if let Some(matrix) = model_bolt_matrix_timed(
                &mut self.perf, &model.glm, &model.gla, &pose, tag)? {
                let point = [matrix[0][3], matrix[1][3], matrix[2][3]];
                anchors[index] = Some(transform_jka_model_point(point, axis, origin));
                if index == 0 {
                    // Face follows the exact same head bolt matrix and its
                    // local axes (rather than a fixed screen-space marker).
                    let face = [point[0] - 5.0 * matrix[0][0] - 7.0 * matrix[0][2],
                                point[1] - 5.0 * matrix[1][0] - 7.0 * matrix[1][2],
                                point[2] - 5.0 * matrix[2][0] - 7.0 * matrix[2][2]];
                    anchors[1] = Some(transform_jka_model_point(face, axis, origin));
                }
            }
        }
        if let Some(profile) = model.jiggle.as_deref() {
            anchors[5] = profile.studio_body_anchor(false, &pose, axis, origin);
            anchors[6] = profile.studio_body_anchor(true, &pose, axis, origin);
        }
        record_pose_eval(&mut self.perf, pose_started);
        // The studio is CPU-skinned for cinematic lighting. Use the same
        // promoted bind-pose mesh as gameplay when drawing its body shape.
        self.profile_body_pick = Some(BodyPick { model: Arc::clone(&model), pose: pose.clone(), axis, origin });
        self.preview_cpu_remesh = true;
        let body_result = self.render_glm_surfaces_tinted(
            0,
            &info.model_qpath(),
            &model.glm,
            &model.gla,
            &model.surfaces,
            self.jiggle_profile_for_sex(model.jiggle.as_deref(), model.female),
            &pose,
            0,
            axis,
            origin,
            [1.0; 4],
            skin_tint,
            None,
            true,
        );
        self.preview_cpu_remesh = false;
        let mut draws = body_result?;

        let diagnostic_enabled = saber_preview_debug_enabled();
        let mut diagnostic_lines = if diagnostic_enabled {
            vec![format!("model={} animation={} time_ms={} emote={} saber0={} saber1={}",
                model.key, animation_name, current_time, emote_preview.is_some(),
                info.saber_name, info.saber2_name)]
        } else { Vec::new() };
        if with_sabers {
            for (saber_num, saber_name, hand_name) in [
                (0usize, info.saber_name.as_str(), "*r_hand"),
                (1usize, info.saber2_name.as_str(), "*l_hand"),
            ] {
                if saber_name.is_empty() || saber_name_is_removed(saber_name) {
                    continue;
                }
                let definition = self.saber_definitions.definition_or_default(saber_name);
                let Some(hand_bolt) = saber_hand_bolt_matrix_timed(
                    &mut self.perf,
                    &model,
                    &pose,
                    hand_name,
                )?
                else {
                    if diagnostic_enabled {
                        diagnostic_lines.push(format!("slot={saber_num} tag={hand_name} MISSING hand bolt"));
                    }
                    continue;
                };
                if diagnostic_enabled {
                    let hand_origin = transform_jka_model_point(
                        [hand_bolt[0][3], hand_bolt[1][3], hand_bolt[2][3]], axis, origin);
                    let hand_axes = preview_saber_axes(&hand_bolt, axis);
                    diagnostic_lines.push(format!(
                        "slot={saber_num} hand={hand_name} hilt={} hand_origin={hand_origin:?} hand_axes_XYZ={hand_axes:?} source_left_grip_half_turn={}",
                        definition.model, model.source_imported && hand_name == "*l_hand"));
                }
                let hilt = self.load_saber_model(&definition)?;
                let mut hilt_animator = Ghoul2Animator::new(&hilt.animation_gla);
                let pose_started = Instant::now();
                let hilt_pose = hilt.bind_animation_pose(hilt_animator.evaluate_pose(&hilt.animation_gla, current_time, hand_bolt)?)?;
                record_pose_eval(&mut self.perf, pose_started);
                if diagnostic_enabled {
                    if let Some(root) = hilt_pose.first() {
                        let hilt_axes = preview_saber_axes(root, axis);
                        diagnostic_lines.push(format!(
                            "slot={saber_num} hilt_pose_root_axes_XYZ={hilt_axes:?} (skeleton bone0, NOT blade tag)"));
                    }
                }
                let mut hilt_draws = self.render_glm_surfaces(
                    0,
                    &definition.model,
                    &hilt.glm,
                    &hilt.gla,
                    &hilt.surfaces,
                    None,
                    &hilt_pose,
                    0,
                    axis,
                    origin,
                    [1.0; 4],
                    None,
                    false,
                )?;
                if self.rt_shadow_casters_enabled {
                    for surface in &mut hilt_draws {
                        if let Some(key) = surface.rt_skinned_key.take() {
                            surface.rt_skinned_key = Some(Arc::<str>::from(format!(
                                "{}#profile-saber{}",
                                key.as_ref(),
                                saber_num,
                            )));
                        }
                    }
                }
                draws.append(&mut hilt_draws);

                // CG_AddSaberBlade-equivalent preview path. Phase 2 attached the
                // hilt but stopped here, which is why the Profile showed a bare
                // handle. Resolve every authored blade bolt exactly as the live
                // player presenter does and hand the resulting blade to the same
                // WeaponFx material/geometry path used in gameplay.
                let client_color = if saber_num == 0 {
                    info.saber_color
                } else {
                    info.saber2_color
                };
                for blade_index in 0..definition.num_blades {
                    let tag_name = format!("*blade{}", blade_index + 1);
                    let (blade_bolt, tag_hack) = if let Some(matrix) = model_bolt_matrix_timed(
                        &mut self.perf,
                        &hilt.glm,
                        &hilt.gla,
                        &hilt_pose,
                        &tag_name,
                    )? {
                        (matrix, false)
                    } else {
                        // UI_SaberDrawBlade falls back to *flash for every
                        // blade, not just blade 0, so pre-JKA hilts still preview.
                        let Some(matrix) = model_bolt_matrix_timed(
                            &mut self.perf,
                            &hilt.glm,
                            &hilt.gla,
                            &hilt_pose,
                            "*flash",
                        )?
                        else {
                            if diagnostic_enabled {
                                diagnostic_lines.push(format!(
                                    "slot={saber_num} blade={blade_index} MISSING {tag_name} and *flash"));
                            }
                            continue;
                        };
                        (matrix, true)
                    };
                    let mut origin_model = [blade_bolt[0][3], blade_bolt[1][3], blade_bolt[2][3]];
                    let mut dir_model =
                        normalize_vec3([-blade_bolt[0][0], -blade_bolt[1][0], -blade_bolt[2][0]]);
                    // Exact stock UI staff tag-hack: when blade2 has no authored
                    // bolt, reverse the *flash direction and offset sixteen units.
                    if tag_hack
                        && blade_index == 1
                        && definition.saber_type.eq_ignore_ascii_case("SABER_STAFF")
                    {
                        dir_model = [-dir_model[0], -dir_model[1], -dir_model[2]];
                        origin_model = [
                            origin_model[0] + dir_model[0] * 16.0,
                            origin_model[1] + dir_model[1] * 16.0,
                            origin_model[2] + dir_model[2] * 16.0,
                        ];
                    }
                    let origin_world = transform_jka_model_point(origin_model, axis, origin);
                    let dir_world = normalize_vec3(transform_jka_model_vector(dir_model, axis));
                    if diagnostic_enabled {
                        let blade_axes = preview_saber_axes(&blade_bolt, axis);
                        let hand_axes = preview_saber_axes(&hand_bolt, axis);
                        // How far did the actual blade deviate from the hand
                        // bolt's expected -X direction? ~180 means an inversion
                        // somewhere between hand bolt and blade bolt.
                        let hand_minus_x = hand_axes[0].map(|v| -v);
                        let dot = (0..3).map(|i| hand_minus_x[i] * dir_world[i]).sum::<f32>()
                            .clamp(-1.0, 1.0);
                        diagnostic_lines.push(format!(
                            "slot={saber_num} blade={blade_index} tag={} blade_origin={origin_world:?} blade_dir={dir_world:?} blade_axes_XYZ={blade_axes:?} handMinusX_to_blade_deg={:.1} worldZ={:.3}",
                            if tag_hack { "*flash" } else { tag_name.as_str() },
                            dot.acos().to_degrees(), dir_world[2]));
                    }
                    let blade = definition.blade(blade_index);
                    let color = blade_color(info, client_color, &blade, None);
                    let secondary_style = definition.blade_style2_start > 0
                        && blade_index >= definition.blade_style2_start;
                    let trail_style = if secondary_style {
                        definition.trail_style2
                    } else {
                        definition.trail_style
                    };
                    let no_wall_marks = if secondary_style {
                        definition.no_wall_marks2
                    } else {
                        definition.no_wall_marks
                    };
                    if let Some(request) = saber_blade_fx_request(
                        origin_world,
                        dir_world,
                        blade.length,
                        blade.length,
                        blade.radius,
                        color,
                        1.0,
                        0,
                        saber_num as u8,
                        blade_index as u8,
                        0,
                        animation_number,
                        false,
                        trail_style,
                        definition.num_blades as u8,
                        definition.no_dlight,
                        no_wall_marks,
                    ) {
                        self.fx_requests.push(request);
                    }
                }
            }
        }

        // The worn jaPRO cosmetics ride the same pose; the caller drains them
        // into the MD3 presenter (see `drain_cosmetic_draws`).
        if cosmetics != 0 {
            let worn = cosmetic_draws_for_mask(
                &mut self.perf,
                &model,
                &pose,
                axis,
                origin,
                cosmetics,
                0,
                [1.0; 4],
                None,
            );
            self.cosmetic_draws.extend(worn);
        }

        if diagnostic_enabled {
            publish_saber_preview_debug(diagnostic_lines.join("\n"));
        }
        apply_profile_studio_light(&mut draws, studio_cinematic);
        Ok((draws, anchors))
    }

    /// TaystJK's Profile saber preview resolves the selected saber block to its
    /// authored saberModel and optional customSkin. Keep that exact data path,
    /// but render it through DinurdoJK's normal Ghoul2/WGPU presenter.
    // Asset-preview entry point that is not wired into the viewer yet.
    #[allow(dead_code)]
    pub fn present_static_saber_preview(
        &mut self,
        saber_name: &str,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        current_time: i32,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let definition = self.saber_definitions.definition_or_default(saber_name);
        let model = self.load_saber_model(&definition)?;
        let pose_started = Instant::now();
        let pose =
            model.bind_animation_pose(Ghoul2Animator::new(&model.animation_gla).evaluate_pose_openjk_root(&model.animation_gla, current_time)?)?;
        record_pose_eval(&mut self.perf, pose_started);
        self.render_glm_surfaces(
            0,
            &definition.model,
            &model.glm,
            &model.gla,
            &model.surfaces,
            None,
            &pose,
            0,
            axis,
            origin,
            [1.0; 4],
            None,
            true,
        )
    }

    /// Developer Asset Viewer GLM path. Player/NPC model.glm files usually
    /// obtain their materials from model_default.skin rather than from the GLM
    /// hierarchy itself. Try that sibling skin automatically. If it is absent
    /// or incomplete, retain renderable geometry with a neutral $whiteimage
    /// material instead of making the model disappear against the black viewer.
    pub fn present_static_glm_preview(
        &mut self,
        entity_num: u16,
        model_qpath: &str,
        origin: [f32; 3],
        axis: [[f32; 3]; 3],
        rgba: [f32; 4],
        current_time: i32,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let normalized = model_qpath.replace('\\', "/");
        let inferred_skin = normalized
            .rsplit_once('/')
            .filter(|(_, leaf)| leaf.eq_ignore_ascii_case("model.glm"))
            .map(|(folder, _)| format!("{folder}/model_default.skin"));
        let model = self.load_static_glm_with_options(
            model_qpath,
            inferred_skin.as_deref(),
            model_qpath,
            true,
        )?;
        let pose_started = Instant::now();
        let pose =
            model.bind_animation_pose(Ghoul2Animator::new(&model.animation_gla).evaluate_pose_openjk_root(&model.animation_gla, current_time)?)?;
        record_pose_eval(&mut self.perf, pose_started);
        self.render_glm_surfaces(
            entity_num,
            model_qpath,
            &model.glm,
            &model.gla,
            &model.surfaces,
            None,
            &pose,
            0,
            axis,
            origin,
            rgba,
            None,
            true,
        )
    }
}

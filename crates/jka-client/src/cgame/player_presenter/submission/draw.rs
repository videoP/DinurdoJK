//! Submission draw.
use crate::cgame::player_presenter::{
    blend_for_alpha, gpu_bones_with_heat_sources, skin_glm_surface, skin_surface_timed, specular_alpha, environment_uv,
    stage_uv_affine, wave_value, Arc, ClothSurfaceFrame, ClothSystem, DynamicModelAlphaMode,
    DynamicModelSurface, DynamicModelVertex, DynamicWireframeClass, Ghoul2GpuSkinning,
    Ghoul2SkinnedSurface, Ghoul2SkinningMode, GlaAnimation, GlmModel, HashMap, HashSet, Instant,
    JiggleProfile, Matrix3x4, PlayerPresenter, PlayerSurfaceAsset, ResolvedStage, StageFrame,
    Ghoul2GpuMeshSource,
    TextureData,
};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

/// CPU preview of exactly the promoted bind-pose mesh used by GPU Ghoul2.
/// Reuses OpenJK's first-bone normal and two-weight position convention;
/// only the isolated Profile takes this path (no in-game CPU skin overhead).
fn skin_profile_promoted_mesh(
    surface_index: usize,
    mesh: &Ghoul2GpuMeshSource,
    pose: &[Matrix3x4],
) -> Result<jka_assets::ghoul2::Ghoul2SkinnedSurface, String> {
    use jka_assets::ghoul2::{transform_normal, transform_point, Ghoul2SkinnedVertex};
    let mut vertices = Vec::with_capacity(mesh.vertices.len());
    for v in mesh.vertices.iter() {
        let count = v.weight_count as usize;
        if !(1..=4).contains(&count) {
            return Err(format!("profile surface {surface_index}: invalid weight count {count}"));
        }
        let matrix = |i: usize| -> Result<&Matrix3x4, String> {
            pose.get(v.bone_indices[i] as usize).ok_or_else(||
                format!("profile surface {surface_index}: missing Ghoul2 bone {}", v.bone_indices[i]))
        };
        let normal = transform_normal(matrix(0)?, v.normal);
        let position = if count == 1 {
            transform_point(matrix(0)?, v.position)
        } else if count == 2 {
            let a = transform_point(matrix(0)?, v.position);
            let b = transform_point(matrix(1)?, v.position);
            std::array::from_fn(|i| v.weights[0] * (a[i] - b[i]) + b[i])
        } else {
            let mut point = [0.0f32; 3];
            let mut sum = 0.0;
            for i in 0..count {
                let weight = if i + 1 == count { 1.0 - sum } else {
                    sum += v.weights[i];
                    v.weights[i]
                };
                let p = transform_point(matrix(i)?, v.position);
                for component in 0..3 { point[component] += weight * p[component]; }
            }
            point
        };
        vertices.push(Ghoul2SkinnedVertex { position, normal, uv: v.uv });
    }
    Ok(jka_assets::ghoul2::Ghoul2SkinnedSurface {
        surface_index,
        vertices,
        indices: mesh.indices.as_ref().clone(),
    })
}

/// Apply the same per-vertex jiggle displacement as ghoul2_skin.wgsl to
/// Profile's CPU-skinned *promoted* mesh. The promoted mesh contains newly
/// subdivided vertices; indexing the original GLM profile mask by their indices
/// would leave those new vertices undeformed (and misassign other weights).
fn deform_profile_promoted_jiggle(
    mesh: &Ghoul2GpuMeshSource,
    offsets: &[[f32; 4]; crate::cgame::jiggle::MAX_GPU_JIGGLE_REGIONS],
    skinned: &mut Ghoul2SkinnedSurface,
) {
    fn smoothstep(lo: f32, hi: f32, value: f32) -> f32 {
        let t = ((value - lo) / (hi - lo)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }
    for (vertex, binding) in skinned.vertices.iter_mut().zip(mesh.vertices.iter()) {
        let region = binding.jiggle_region as usize;
        if region >= offsets.len() || binding.jiggle_weight <= 0.0 {
            continue;
        }
        let mut weight = binding.jiggle_weight;
        if binding.jiggle_coord.abs() < 2.0 {
            let bottom = offsets[0][3];
            let top = offsets[1][3];
            let lower = smoothstep(-0.70 + bottom, -0.15 + bottom, binding.jiggle_coord);
            let upper = if top < 0.0 {
                1.0 - smoothstep(0.65 + top, 1.0 + top, binding.jiggle_coord)
            } else {
                1.0
            };
            weight *= lower * upper;
        }
        for component in 0..3 {
            vertex.position[component] += offsets[region][component] * weight;
        }
    }
}

impl PlayerPresenter {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::cgame::player_presenter) fn render_glm_surfaces(
        &mut self,
        entity_num: u16,
        model_label: &str,
        glm: &GlmModel,
        gla: &GlaAnimation,
        surface_assets: &[PlayerSurfaceAsset],
        jiggle_profile: Option<&JiggleProfile>,
        pose: &[Matrix3x4],
        lod_index: usize,
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        rgba: [f32; 4],
        custom_material: Option<(Option<Arc<TextureData>>, DynamicModelAlphaMode, bool)>,
        apply_alpha_blend: bool,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        self.render_glm_surfaces_tinted(
            entity_num,
            model_label,
            glm,
            gla,
            surface_assets,
            jiggle_profile,
            pose,
            lod_index,
            axis,
            origin,
            rgba,
            [1.0; 3],
            custom_material,
            apply_alpha_blend,
        )
    }

    /// `render_glm_surfaces` plus the refEntity `shaderRGBA` tint of a player
    /// (`customRGBA`, i.e. `char_color_*`). Only shaders that read the entity
    /// colour (`rgbGen lightingDiffuseEntity`) follow it; their untinted overlay
    /// stage is drawn over them, so just the texture's alpha-masked regions change.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::cgame::player_presenter) fn render_glm_surfaces_tinted(
        &mut self,
        entity_num: u16,
        model_label: &str,
        glm: &GlmModel,
        gla: &GlaAnimation,
        surface_assets: &[PlayerSurfaceAsset],
        jiggle_profile: Option<&JiggleProfile>,
        pose: &[Matrix3x4],
        lod_index: usize,
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        rgba: [f32; 4],
        entity_rgb: [f32; 3],
        custom_material: Option<(Option<Arc<TextureData>>, DynamicModelAlphaMode, bool)>,
        apply_alpha_blend: bool,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        self.render_glm_surfaces_tinted_selected(
            entity_num,
            model_label,
            glm,
            gla,
            surface_assets,
            jiggle_profile,
            pose,
            lod_index,
            axis,
            origin,
            rgba,
            entity_rgb,
            custom_material,
            apply_alpha_blend,
            None,
        )
    }

    pub(in crate::cgame::player_presenter) fn render_glm_surfaces_tinted_selected(
        &mut self,
        entity_num: u16,
        model_label: &str,
        glm: &GlmModel,
        gla: &GlaAnimation,
        surface_assets: &[PlayerSurfaceAsset],
        jiggle_profile: Option<&JiggleProfile>,
        pose: &[Matrix3x4],
        lod_index: usize,
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        rgba: [f32; 4],
        entity_rgb: [f32; 3],
        custom_material: Option<(Option<Arc<TextureData>>, DynamicModelAlphaMode, bool)>,
        apply_alpha_blend: bool,
        surface_selection: Option<&HashSet<usize>>,
    ) -> Result<Vec<DynamicModelSurface>, String> {
        let tinted = entity_rgb != [1.0; 3] && custom_material.is_none();
        let stage_seconds = self.stage_time_ms as f32 * 0.001;
        let stage_frame = StageFrame {
            axis,
            origin,
            viewer: self.stage_view_position,
        };
        let lod_index = lod_index.min(glm.lods.len().saturating_sub(1));
        let lod = glm
            .lods
            .get(lod_index)
            .ok_or_else(|| format!("{model_label} has no GLM LODs"))?;
        let jobs = surface_assets
            .iter()
            .filter_map(|asset| {
                let visible = surface_selection.map_or(asset.default_visible, |selection| {
                    selection.contains(&asset.surface_index)
                });
                if !visible {
                    return None;
                }
                let surface = lod
                    .surfaces
                    .iter()
                    .find(|surface| surface.surface_index == asset.surface_index)?;
                let gpu_mesh = asset.gpu_meshes.get(lod_index)?.as_ref()?;
                Some((asset, surface, gpu_mesh))
            })
            .collect::<Vec<_>>();

        let jiggle_profile = jiggle_profile.filter(|_| self.jiggle.enabled());
        let jiggle_offsets = jiggle_profile.map(|profile| {
            self.jiggle.simulate(
                entity_num,
                model_label,
                profile,
                pose,
                axis,
                origin,
                self.stage_time_ms,
            )
        });
        let gpu_jiggle_offsets =
            if let (Some(profile), Some(offsets)) = (jiggle_profile, jiggle_offsets.as_deref()) {
                if profile.gpu_supported() {
                    profile.gpu_offsets(offsets, self.jiggle.tuning())
                } else {
                    Default::default()
                }
            } else {
                Default::default()
            };

        if self.skinning_mode == Ghoul2SkinningMode::Gpu {
            let (bones, heat_bone) = gpu_bones_with_heat_sources(pose, gla);
            let empty_vertices = Arc::new(Vec::new());

            // Cloth remains a CPU post-skin deformation path. Jiggle stays on
            // GPU whenever its profile fits the four-region vertex payload; only
            // cloth (or an oversized explicit jiggle profile) falls back to CPU.
            let mut deformed_skinned = HashMap::<usize, Ghoul2SkinnedSurface>::new();
            let mut cloth_frames = Vec::<ClothSurfaceFrame>::new();
            for (_, surface, _) in &jobs {
                let cloth_candidate = self.cloth.enabled()
                    && Self::cloth_surface_name(glm, surface)
                        .is_some_and(ClothSystem::is_cloth_surface_name);
                let jiggle_candidate = jiggle_profile.is_some_and(|profile| {
                    profile.affects_surface(lod_index, surface.surface_index)
                });
                let jiggle_cpu_fallback = jiggle_candidate
                    && jiggle_profile.is_some_and(|profile| !profile.gpu_supported());
                if !cloth_candidate && !jiggle_cpu_fallback {
                    continue;
                }

                let mut skinned = skin_surface_timed(&mut self.perf, surface, pose)?;
                if jiggle_candidate {
                    if let (Some(profile), Some(offsets)) =
                        (jiggle_profile, jiggle_offsets.as_deref())
                    {
                        profile.deform_surface(
                            lod_index,
                            surface.surface_index,
                            offsets,
                            self.jiggle.tuning(),
                            &mut skinned,
                        );
                    }
                }
                if cloth_candidate {
                    if let Some(frame) = Self::cloth_surface_frame(glm, surface, &skinned, pose) {
                        cloth_frames.push(frame);
                    }
                }
                deformed_skinned.insert(surface.surface_index, skinned);
            }
            let mut cloth_outputs = self.simulate_cloth_frames(
                entity_num,
                model_label,
                lod_index,
                cloth_frames,
                glm,
                surface_assets,
                gla,
                pose,
                axis,
                origin,
            );

            let mut draws = Vec::with_capacity(jobs.len());
            for (asset, surface, gpu_mesh) in jobs {
                let gpu_jiggle_surface = jiggle_profile.is_some_and(|profile| {
                    profile.gpu_supported()
                        && profile.affects_surface(lod_index, surface.surface_index)
                });
                let (draw_key, draw_vertices, draw_indices) = if gpu_jiggle_surface {
                    (&gpu_mesh.key, &gpu_mesh.vertices, &gpu_mesh.indices)
                } else {
                    (
                        &gpu_mesh.base_key,
                        &gpu_mesh.base_vertices,
                        &gpu_mesh.cpu_indices,
                    )
                };
                if draw_vertices.is_empty() || draw_indices.is_empty() {
                    continue;
                }
                let (texture, base_alpha_mode, authored_glow) = custom_material
                    .clone()
                    .unwrap_or_else(|| (asset.texture.clone(), asset.alpha_mode, asset.authored_glow));
                let surface_rgba = if custom_material.is_none() && asset.fallback_gray {
                    [0.58, 0.58, 0.58, rgba[3]]
                } else if tinted && asset.entity_tint {
                    [entity_rgb[0], entity_rgb[1], entity_rgb[2], rgba[3]]
                } else {
                    rgba
                };
                let alpha_mode = if apply_alpha_blend {
                    blend_for_alpha(base_alpha_mode, surface_rgba[3])
                } else {
                    base_alpha_mode
                };
                let overlay_at = draws.len() + 1;

                if let Some(skinned) = deformed_skinned.remove(&surface.surface_index) {
                    let vertices = Self::cpu_surface_vertices_with_cloth(
                        skinned,
                        cloth_outputs.remove(&surface.surface_index),
                        axis,
                        origin,
                        surface_rgba,
                    );
                    if !vertices.is_empty() {
                        draws.push(DynamicModelSurface {
                            geometry_source: crate::renderer::ModelGeometrySource::Ghoul2,
                            authored_lighting: false,
                            material_trace: None,
                            entity_num,
                            wireframe_class: DynamicWireframeClass::Player,
                            raster_visible: true,
                            vertices: Arc::new(vertices),
                            indices: Arc::clone(&gpu_mesh.cpu_indices),
                            lighting_origin: Some(origin),
                            rt_rigid: None,
                            rt_skinned_key: self
                                .rt_shadow_casters_enabled
                                .then(|| Arc::clone(&gpu_mesh.base_key)),
                            ghoul2_gpu: None,
                            fx_gpu_sprites: None,
                            authored_glow,
                            texture,
                            alpha_mode,
                        });
                        if tinted && asset.entity_tint {
                            Self::push_tint_overlay(&mut draws, overlay_at, asset, rgba[3]);
                        }
                        if custom_material.is_none() {
                            Self::push_stage_draws(
                                &mut draws,
                                overlay_at,
                                asset,
                                rgba[3],
                                jka_assets::shader::shader_time(stage_seconds, asset.shader_time_offset, asset.shader_clamp_time),
                                &stage_frame,
                            );
                        }
                    }
                    continue;
                }

                self.perf.surfaces_skinned = self.perf.surfaces_skinned.saturating_add(1);
                self.perf.vertices_skinned = self
                    .perf
                    .vertices_skinned
                    .saturating_add(draw_vertices.len() as u64);
                draws.push(DynamicModelSurface {
                    geometry_source: crate::renderer::ModelGeometrySource::Ghoul2,
                    authored_lighting: false,
                    material_trace: None,
                    entity_num,
                    wireframe_class: DynamicWireframeClass::Player,
                    raster_visible: true,
                    vertices: Arc::clone(&empty_vertices),
                    indices: Arc::clone(draw_indices),
                    lighting_origin: Some(origin),
                    rt_rigid: None,
                    rt_skinned_key: self.rt_shadow_casters_enabled.then(|| Arc::clone(draw_key)),
                    ghoul2_gpu: Some(Ghoul2GpuSkinning { foliage_wind: false,
                        mesh_key: Arc::clone(draw_key),
                        vertices: Arc::clone(draw_vertices),
                        indices: Arc::clone(draw_indices),
                        bones: Arc::clone(&bones),
                        heat_bone,
                        axis,
                        origin,
                        color: surface_rgba,
                        uv_xform: [1.0, 1.0, 0.0, 0.0],
                        uv_cross: [0.0; 4],
                        specular: None,
                        bulge_height: 0.0,
                        env_map: false,
                        jiggle_offsets: gpu_jiggle_offsets,
                        // A customShader/tint stage replaces the material, so the
                        // VMT's bump and phong only ride along with the base one.
                        source_material: custom_material
                            .is_none()
                            .then(|| asset.source_material.clone())
                            .flatten(),
                    }),
                    fx_gpu_sprites: None,
                    authored_glow,
                    texture,
                    alpha_mode,
                });
                if tinted && asset.entity_tint {
                    Self::push_tint_overlay(&mut draws, overlay_at, asset, rgba[3]);
                }
                if custom_material.is_none() {
                    Self::push_stage_draws(
                        &mut draws,
                        overlay_at,
                        asset,
                        rgba[3],
                        jka_assets::shader::shader_time(stage_seconds, asset.shader_time_offset, asset.shader_clamp_time),
                        &stage_frame,
                    );
                }
            }
            return Ok(draws);
        }

        let preview_remesh = self.preview_cpu_remesh && self.jiggle_remesh != Default::default();
        let used_workers = !preview_remesh && self.skinning_mode == Ghoul2SkinningMode::CpuWorkers && jobs.len() > 1;
        let mut skinned = if used_workers {
            let started = Instant::now();
            let results = self.skinning_pool.install(|| {
                jobs.par_iter()
                    .map(|(_, surface, _)| skin_glm_surface(surface, pose))
                    .collect::<Vec<_>>()
            });
            self.perf.skin_ms += started.elapsed().as_secs_f64() * 1000.0;
            results
        } else {
            jobs.iter()
                .map(|(_, surface, mesh)| {
                    if preview_remesh && mesh.key != mesh.base_key {
                        skin_profile_promoted_mesh(surface.surface_index, mesh, pose)
                    } else {
                        skin_surface_timed(&mut self.perf, surface, pose)
                    }
                })
                .collect::<Vec<_>>()
        };

        if let (Some(profile), Some(offsets)) = (jiggle_profile, jiggle_offsets.as_deref()) {
            for ((_, surface, mesh), result) in jobs.iter().zip(skinned.iter_mut()) {
                if !profile.affects_surface(lod_index, surface.surface_index) {
                    continue;
                }
                if let Ok(skinned_surface) = result {
                    if preview_remesh && mesh.key != mesh.base_key {
                        // Promoted topology: use the bindings interpolated for
                        // *each* new vertex, just like the gameplay GPU path.
                        deform_profile_promoted_jiggle(mesh, &gpu_jiggle_offsets, skinned_surface);
                    } else {
                        profile.deform_surface(
                            lod_index,
                            surface.surface_index,
                            offsets,
                            self.jiggle.tuning(),
                            skinned_surface,
                        );
                    }
                }
            }
        }

        let cloth_frames = if self.cloth.enabled() {
            jobs.iter()
                .zip(skinned.iter())
                .filter_map(|((_, surface, _), skinned)| {
                    let skinned = skinned.as_ref().ok()?;
                    Self::cloth_surface_frame(glm, surface, skinned, pose)
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let mut cloth_outputs = self.simulate_cloth_frames(
            entity_num,
            model_label,
            lod_index,
            cloth_frames,
            glm,
            surface_assets,
            gla,
            pose,
            axis,
            origin,
        );

        let mut draws = Vec::with_capacity(jobs.len());
        for ((asset, surface, gpu_mesh), skinned) in jobs.into_iter().zip(skinned) {
            let skinned = skinned?;
            let surface_rgba = if custom_material.is_none() && asset.fallback_gray {
                [0.58, 0.58, 0.58, rgba[3]]
            } else if tinted && asset.entity_tint {
                [entity_rgb[0], entity_rgb[1], entity_rgb[2], rgba[3]]
            } else {
                rgba
            };
            if used_workers {
                self.perf.surfaces_skinned = self.perf.surfaces_skinned.saturating_add(1);
                self.perf.vertices_skinned = self
                    .perf
                    .vertices_skinned
                    .saturating_add(skinned.vertices.len() as u64);
            }
            let vertices = Self::cpu_surface_vertices_with_cloth(
                skinned,
                cloth_outputs.remove(&surface.surface_index),
                axis,
                origin,
                surface_rgba,
            );
            let draw_indices = if preview_remesh && gpu_mesh.key != gpu_mesh.base_key {
                &gpu_mesh.indices
            } else {
                &gpu_mesh.cpu_indices
            };
            if vertices.is_empty() || draw_indices.is_empty() {
                continue;
            }
            let (texture, base_alpha_mode, authored_glow) = custom_material
                .clone()
                .unwrap_or_else(|| (asset.texture.clone(), asset.alpha_mode, asset.authored_glow));
            let alpha_mode = if apply_alpha_blend {
                blend_for_alpha(base_alpha_mode, surface_rgba[3])
            } else {
                base_alpha_mode
            };
            let overlay_at = draws.len() + 1;
            draws.push(DynamicModelSurface {
                geometry_source: crate::renderer::ModelGeometrySource::Ghoul2,
                authored_lighting: false,
                material_trace: None,
                entity_num,
                wireframe_class: DynamicWireframeClass::Player,
                raster_visible: true,
                vertices: Arc::new(vertices),
                indices: Arc::clone(draw_indices),
                lighting_origin: Some(origin),
                rt_rigid: None,
                rt_skinned_key: self
                    .rt_shadow_casters_enabled
                    .then(|| Arc::clone(&gpu_mesh.base_key)),
                ghoul2_gpu: None,
                fx_gpu_sprites: None,
                authored_glow,
                texture,
                alpha_mode,
            });
            self.push_profile_selection(&mut draws, overlay_at - 1, gpu_mesh,
                preview_remesh && gpu_mesh.key != gpu_mesh.base_key);
            if tinted && asset.entity_tint {
                Self::push_tint_overlay(&mut draws, overlay_at, asset, rgba[3]);
            }
            if custom_material.is_none() {
                Self::push_stage_draws(
                    &mut draws,
                    overlay_at,
                    asset,
                    rgba[3],
                    jka_assets::shader::shader_time(stage_seconds, asset.shader_time_offset, asset.shader_clamp_time),
                    &stage_frame,
                );
            }
        }
        Ok(draws)
    }

    /// Turn the surface draw just pushed (`draws[base_at - 1]`) into the first
    /// stage of its blended shader and add one draw per further stage. Shaders
    /// without a stage list (everything opaque/masked) leave the draw alone.
    /// The extra draws repeat geometry that is already a shadow caster.
    pub(in crate::cgame::player_presenter) fn push_stage_draws(
        draws: &mut Vec<DynamicModelSurface>,
        base_at: usize,
        asset: &PlayerSurfaceAsset,
        alpha: f32,
        seconds: f32,
        frame: &StageFrame,
    ) {
        let Some(first) = asset.stages.first() else {
            return;
        };
        let Some(base) = draws.get(base_at - 1) else {
            return;
        };
        let extras = asset
            .stages
            .iter()
            .skip(1)
            .map(|stage| {
                let mut surface = Self::clone_surface_with_material(
                    base,
                    stage.texture.clone(),
                    stage.alpha_mode,
                    [1.0, 1.0, 1.0, alpha],
                );
                surface.rt_rigid = None;
                surface.rt_skinned_key = None;
                Self::apply_stage(&mut surface, stage, alpha, seconds, frame);
                surface
            })
            .collect::<Vec<_>>();
        Self::apply_stage(&mut draws[base_at - 1], first, alpha, seconds, frame);
        draws.extend(extras);
    }

    /// Colour, texture-coordinate transform and lighting of one shader stage.
    /// CPU vertices are rewritten from the surface's own (untransformed) UVs.
    pub(in crate::cgame::player_presenter) fn apply_stage(
        surface: &mut DynamicModelSurface,
        stage: &ResolvedStage,
        alpha: f32,
        seconds: f32,
        frame: &StageFrame,
    ) {
        // TaystJK RB_CalcWaveColor/RB_CalcWaveAlpha clamp each waveform to
        // [0,1] at shader time. In particular, alphaGen wave MUST NOT become
        // identity on the first blended depth-writing pass.
        let rgb = stage
            .rgb_wave
            .map(|wave| wave_value(wave, seconds).clamp(0.0, 1.0));
        let generated_alpha = stage
            .alpha_wave
            .map(|wave| wave_value(wave, seconds).clamp(0.0, 1.0))
            .unwrap_or(stage.alpha);
        let mut color = [
            rgb.unwrap_or(stage.rgb[0]),
            rgb.unwrap_or(stage.rgb[1]),
            rgb.unwrap_or(stage.rgb[2]),
            alpha * generated_alpha,
        ];
        let affine = stage_uv_affine(&stage.tc_mods, seconds);
        let xform = [affine[0], affine[3], affine[4], affine[5]];
        let cross = [affine[2], affine[1], 0.0, 0.0];
        let specular = frame.specular_points().filter(|_| stage.specular_alpha);
        if stage.specular_alpha && specular.is_none() {
            // No viewer (asset preview): a constant mid glint instead of none.
            color[3] *= 0.5;
        }
        surface.texture = stage.texture.clone();
        surface.alpha_mode = stage.alpha_mode;
        surface.authored_glow = stage.authored_glow;
        // Without a viewer (asset previews) reflect a camera straight ahead.
        let env_viewer = stage.env_map.then(|| frame.viewer.unwrap_or_else(|| {
            std::array::from_fn(|i| frame.origin[i] + frame.axis[0][i] * 4096.0)
        }));
        if let Some(skin) = surface.ghoul2_gpu.as_mut() {
            skin.color = color;
            skin.uv_xform = xform;
            skin.uv_cross = cross;
            skin.specular = specular;
            skin.env_map = stage.env_map;
        } else {
            surface.vertices = Arc::new(
                surface
                    .vertices
                    .iter()
                    .map(|vertex| {
                        let mut color = color;
                        if let Some((light, viewer)) = specular {
                            color[3] *=
                                specular_alpha(vertex.position, vertex.normal, light, viewer);
                        }
                        let uv = match env_viewer {
                            Some(viewer) => environment_uv(vertex.position, vertex.normal, viewer),
                            None => vertex.uv,
                        };
                        DynamicModelVertex { foliage_wind: 0.0,
                            uv: [
                                uv[0] * xform[0] + uv[1] * cross[0] + xform[2],
                                uv[0] * cross[1] + uv[1] * xform[1] + xform[3],
                            ],
                            color,
                            ..*vertex
                        }
                    })
                    .collect(),
            );
        }
        if stage.unlit {
            surface.lighting_origin = None;
        }
    }

    /// Redraw the tinted surface just pushed (`draws[overlay_at - 1]`) with the
    /// shader's untinted alpha-blended stage. It only repeats geometry that is
    /// already a shadow caster, so it carries no shadow keys of its own.
    pub(in crate::cgame::player_presenter) fn push_tint_overlay(
        draws: &mut Vec<DynamicModelSurface>,
        overlay_at: usize,
        asset: &PlayerSurfaceAsset,
        alpha: f32,
    ) {
        let Some(overlay) = &asset.overlay else {
            return;
        };
        let Some(base) = draws.get(overlay_at - 1) else {
            return;
        };
        let mut surface = Self::clone_surface_with_material(
            base,
            overlay.texture.clone(),
            overlay.alpha_mode,
            [1.0, 1.0, 1.0, alpha],
        );
        surface.rt_rigid = None;
        surface.rt_skinned_key = None;
        surface.authored_glow = overlay.authored_glow;
        draws.push(surface);
    }

    pub(in crate::cgame::player_presenter) fn clone_surface_with_material(
        surface: &DynamicModelSurface,
        texture: Option<Arc<TextureData>>,
        alpha_mode: DynamicModelAlphaMode,
        color: [f32; 4],
    ) -> DynamicModelSurface {
        Self::clone_surface_with_material_xform(
            surface, texture, alpha_mode, color, None, None, None,
        )
    }

    /// `clone_surface_with_material`, optionally overriding the UV transform,
    /// bulge offset and environment mapping instead of inheriting the base
    /// surface's (a customShader's own `tcMod`/`deformVertexes bulge`/`tcGen
    /// environment` reads the base mesh's natural UVs and normals, same as the
    /// engine).
    #[allow(clippy::too_many_arguments)]
    pub(in crate::cgame::player_presenter) fn clone_surface_with_material_xform(
        surface: &DynamicModelSurface,
        texture: Option<Arc<TextureData>>,
        alpha_mode: DynamicModelAlphaMode,
        color: [f32; 4],
        uv_xform: Option<[f32; 6]>,
        bulge_height: Option<f32>,
        env_map: Option<bool>,
    ) -> DynamicModelSurface {
        let ghoul2_gpu = surface.ghoul2_gpu.as_ref().map(|skin| Ghoul2GpuSkinning { foliage_wind: skin.foliage_wind,
            mesh_key: Arc::clone(&skin.mesh_key),
            vertices: Arc::clone(&skin.vertices),
            indices: Arc::clone(&skin.indices),
            bones: Arc::clone(&skin.bones),
            heat_bone: skin.heat_bone,
            axis: skin.axis,
            origin: skin.origin,
            color,
            uv_xform: uv_xform.map_or(skin.uv_xform, |m| [m[0], m[3], m[4], m[5]]),
            uv_cross: uv_xform.map_or(skin.uv_cross, |m| [m[2], m[1], 0.0, 0.0]),
            specular: skin.specular,
            bulge_height: bulge_height.unwrap_or(skin.bulge_height),
            env_map: env_map.unwrap_or(skin.env_map),
            jiggle_offsets: skin.jiggle_offsets,
            // This clone is a different material (overlay, stage or
            // customShader), so it keeps none of the base one's VMT lighting.
            source_material: None,
        });
        let vertices = if ghoul2_gpu.is_some() {
            Arc::clone(&surface.vertices)
        } else {
            Arc::new(
                surface
                    .vertices
                    .iter()
                    .map(|vertex| DynamicModelVertex { foliage_wind: vertex.foliage_wind,
                        uv: match uv_xform {
                            Some(m) => [
                                vertex.uv[0] * m[0] + vertex.uv[1] * m[2] + m[4],
                                vertex.uv[0] * m[1] + vertex.uv[1] * m[3] + m[5],
                            ],
                            None => vertex.uv,
                        },
                        color,
                        ..*vertex
                    })
                    .collect(),
            )
        };
        DynamicModelSurface {
            geometry_source: surface.geometry_source,
            authored_lighting: false,
            material_trace: None,
            entity_num: surface.entity_num,
            wireframe_class: DynamicWireframeClass::Player,
            raster_visible: surface.raster_visible,
            vertices,
            indices: Arc::clone(&surface.indices),
            lighting_origin: surface.lighting_origin,
            rt_rigid: surface.rt_rigid.clone(),
            rt_skinned_key: surface.rt_skinned_key.as_ref().map(Arc::clone),
            ghoul2_gpu,
            fx_gpu_sprites: None,
            authored_glow: surface.authored_glow,
            texture,
            alpha_mode,
        }
    }
}

#[cfg(test)]
mod shader_time_tests {
    use jka_assets::shader::shader_time as shader_stage_time;

    #[test]
    fn ghoul2_clamptime_matches_taystjk_after_remap_offset() {
        assert_eq!(shader_stage_time(9.0, 2.0, Some(3.0)), 3.0);
        assert_eq!(shader_stage_time(4.0, 2.0, Some(3.0)), 2.0);
        assert_eq!(shader_stage_time(9.0, 2.0, None), 7.0);
        assert_eq!(shader_stage_time(1.0, 2.0, Some(3.0)), -1.0);
    }
}

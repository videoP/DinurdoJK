//! Submission draw.
use crate::cgame::player_presenter::{
    blend_for_alpha, gpu_bones_from_pose, skin_glm_surface, skin_surface_timed, specular_alpha,
    stage_uv_xform, Arc, ClothSurfaceFrame, ClothSystem, DynamicModelAlphaMode,
    DynamicModelSurface, DynamicModelVertex, DynamicWireframeClass, Ghoul2GpuSkinning,
    Ghoul2SkinnedSurface, Ghoul2SkinningMode, GlaAnimation, GlmModel, HashMap, HashSet, Instant,
    JiggleProfile, Matrix3x4, PlayerPresenter, PlayerSurfaceAsset, ResolvedStage, StageFrame,
    TextureData,
};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

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
        custom_material: Option<(Option<Arc<TextureData>>, DynamicModelAlphaMode)>,
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
        custom_material: Option<(Option<Arc<TextureData>>, DynamicModelAlphaMode)>,
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
        custom_material: Option<(Option<Arc<TextureData>>, DynamicModelAlphaMode)>,
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
                    [[0.0; 4]; 4]
                }
            } else {
                [[0.0; 4]; 4]
            };

        if self.skinning_mode == Ghoul2SkinningMode::Gpu {
            let bones = gpu_bones_from_pose(pose);
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
                &cloth_frames,
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
                let (texture, base_alpha_mode) = custom_material
                    .clone()
                    .unwrap_or_else(|| (asset.texture.clone(), asset.alpha_mode));
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
                                stage_seconds,
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
                    entity_num,
                    wireframe_class: DynamicWireframeClass::Player,
                    raster_visible: true,
                    vertices: Arc::clone(&empty_vertices),
                    indices: Arc::clone(draw_indices),
                    lighting_origin: Some(origin),
                    rt_rigid: None,
                    rt_skinned_key: self.rt_shadow_casters_enabled.then(|| Arc::clone(draw_key)),
                    ghoul2_gpu: Some(Ghoul2GpuSkinning {
                        mesh_key: Arc::clone(draw_key),
                        vertices: Arc::clone(draw_vertices),
                        indices: Arc::clone(draw_indices),
                        bones: Arc::clone(&bones),
                        axis,
                        origin,
                        color: surface_rgba,
                        uv_xform: [1.0, 1.0, 0.0, 0.0],
                        specular: None,
                        bulge_height: 0.0,
                        env_map: false,
                        jiggle_offsets: gpu_jiggle_offsets,
                    }),
                    fx_gpu_sprites: None,
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
                        stage_seconds,
                        &stage_frame,
                    );
                }
            }
            return Ok(draws);
        }

        let used_workers = self.skinning_mode == Ghoul2SkinningMode::CpuWorkers && jobs.len() > 1;
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
                .map(|(_, surface, _)| skin_surface_timed(&mut self.perf, surface, pose))
                .collect::<Vec<_>>()
        };

        if let (Some(profile), Some(offsets)) = (jiggle_profile, jiggle_offsets.as_deref()) {
            for ((_, surface, _), result) in jobs.iter().zip(skinned.iter_mut()) {
                if !profile.affects_surface(lod_index, surface.surface_index) {
                    continue;
                }
                if let Ok(skinned_surface) = result {
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
            &cloth_frames,
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
            if vertices.is_empty() || gpu_mesh.cpu_indices.is_empty() {
                continue;
            }
            let (texture, base_alpha_mode) = custom_material
                .clone()
                .unwrap_or_else(|| (asset.texture.clone(), asset.alpha_mode));
            let alpha_mode = if apply_alpha_blend {
                blend_for_alpha(base_alpha_mode, surface_rgba[3])
            } else {
                base_alpha_mode
            };
            let overlay_at = draws.len() + 1;
            draws.push(DynamicModelSurface {
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
                    stage_seconds,
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
        let mut color = [
            stage.rgb[0],
            stage.rgb[1],
            stage.rgb[2],
            alpha * stage.alpha,
        ];
        let xform = stage_uv_xform(&stage.tc_mods, seconds);
        let specular = frame.specular_points().filter(|_| stage.specular_alpha);
        if stage.specular_alpha && specular.is_none() {
            // No viewer (asset preview): a constant mid glint instead of none.
            color[3] *= 0.5;
        }
        surface.texture = stage.texture.clone();
        surface.alpha_mode = stage.alpha_mode;
        if let Some(skin) = surface.ghoul2_gpu.as_mut() {
            skin.color = color;
            skin.uv_xform = xform;
            skin.specular = specular;
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
                        DynamicModelVertex {
                            uv: [
                                vertex.uv[0] * xform[0] + xform[2],
                                vertex.uv[1] * xform[1] + xform[3],
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
        uv_xform: Option<[f32; 4]>,
        bulge_height: Option<f32>,
        env_map: Option<bool>,
    ) -> DynamicModelSurface {
        let ghoul2_gpu = surface.ghoul2_gpu.as_ref().map(|skin| Ghoul2GpuSkinning {
            mesh_key: Arc::clone(&skin.mesh_key),
            vertices: Arc::clone(&skin.vertices),
            indices: Arc::clone(&skin.indices),
            bones: Arc::clone(&skin.bones),
            axis: skin.axis,
            origin: skin.origin,
            color,
            uv_xform: uv_xform.unwrap_or(skin.uv_xform),
            specular: skin.specular,
            bulge_height: bulge_height.unwrap_or(skin.bulge_height),
            env_map: env_map.unwrap_or(skin.env_map),
            jiggle_offsets: skin.jiggle_offsets,
        });
        let vertices = if ghoul2_gpu.is_some() {
            Arc::clone(&surface.vertices)
        } else {
            Arc::new(
                surface
                    .vertices
                    .iter()
                    .map(|vertex| DynamicModelVertex {
                        uv: match uv_xform {
                            Some(xform) => [
                                vertex.uv[0] * xform[0] + xform[2],
                                vertex.uv[1] * xform[1] + xform[3],
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
            texture,
            alpha_mode,
        }
    }
}

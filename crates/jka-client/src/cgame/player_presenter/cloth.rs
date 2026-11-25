//! Cloth.
use crate::cgame::player_presenter::{
    scene, submission::mesh::append_surface_indices, transform_model_normal,
    transform_model_point, Arc, ClothCapsule, ClothMotion, ClothOutput, ClothSurfaceFrame,
    ClothSystem, DynamicModelVertex, Ghoul2GpuMeshSource, Ghoul2SkinnedSurface, GlaAnimation,
    GlmModel, GlmSurface, HashMap, Matrix3x4, PlayerPresenter, PlayerSurfaceAsset,
};

/// A GPU cloth garment build (r_clothSolver 1), keyed by model/LOD/surfaces.
pub(in crate::cgame::player_presenter) enum ClothGpuGarmentSlot {
    Pending(std::sync::mpsc::Receiver<Result<crate::renderer::ClothGpuGarment, String>>),
    Ready(Arc<crate::renderer::ClothGpuGarment>),
    Failed,
}

/// One GPU cloth render surface: garment surface ordinal, draw indices and
/// the stable RT-caster key of its (remeshed) topology.
pub(in crate::cgame::player_presenter) struct ClothGpuDraw {
    pub(in crate::cgame::player_presenter) ordinal: u32,
    pub(in crate::cgame::player_presenter) indices: Arc<Vec<u32>>,
    pub(in crate::cgame::player_presenter) key: Arc<str>,
}

/// Densified bind-pose cloth surface plus its draw topology. Only used while
/// cloth physics is on; the stock GLM surface draws otherwise.
pub(in crate::cgame::player_presenter) struct ClothRemesh {
    pub(in crate::cgame::player_presenter) surface: GlmSurface,
    /// Same winding/two-sided convention as `Ghoul2GpuMeshSource::cpu_indices`.
    pub(in crate::cgame::player_presenter) indices: Arc<Vec<u32>>,
    /// Distinct RT skinned-caster key: the vertex count differs from the base.
    pub(in crate::cgame::player_presenter) key: Arc<str>,
}

impl PlayerPresenter {
    /// Cached remesh for one auto-cloth surface, built on first use.
    pub(in crate::cgame::player_presenter) fn cloth_remesh(
        &mut self,
        surface: &GlmSurface,
        gpu_mesh: &Ghoul2GpuMeshSource,
    ) -> Option<Arc<ClothRemesh>> {
        if let Some(cached) = self.cloth_remeshes.get(&gpu_mesh.base_key) {
            return cached.clone();
        }
        let two_sided = gpu_mesh.cpu_indices.len() > surface.triangles.len() * 3;
        let built = crate::cgame::cloth_remesh::remesh_cloth_surface(surface).and_then(|surface| {
            let indices = append_surface_indices(&surface, two_sided).ok()?;
            Some(Arc::new(ClothRemesh {
                key: Arc::from(format!("{}#cloth_remesh_v1", gpu_mesh.base_key)),
                indices: Arc::new(indices),
                surface,
            }))
        });
        self.cloth_remeshes
            .insert(Arc::clone(&gpu_mesh.base_key), built.clone());
        built
    }

    /// Body proxies for promoted GLM hair springs (cg_hairPhysics 2), the same
    /// capsules cloth uses, fitted at LOD 0 without the hair surfaces. Imported
    /// Source rigs get none: their jigglebones are not necessarily hair.
    pub(in crate::cgame::player_presenter) fn hair_spring_colliders(
        templates: &mut HashMap<(String, usize, Vec<usize>), Vec<(usize, ClothCapsule)>>,
        model: &crate::cgame::player_presenter::PlayerModelAsset,
        pose: &[Matrix3x4],
    ) -> Vec<jka_assets::source1_jiggle::SpringCollider> {
        if model.source_imported {
            return Vec::new();
        }
        let visible = model
            .surfaces
            .iter()
            .map(|asset| asset.surface_index)
            .filter(|&index| {
                !model.glm.hierarchy.get(index).is_some_and(|entry| {
                    entry.name.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("hair"))
                })
            })
            .collect::<Vec<_>>();
        let key = (format!("{}#hair_springs", model.key), 0, visible);
        let templates = templates.entry(key.clone()).or_insert_with(|| {
            crate::cgame::cloth_body::build_body_templates(&model.glm, &model.gla, &key.2, 0)
        });
        crate::cgame::cloth_body::pose_body_capsules(templates, &model.gla, pose)
            .into_iter()
            .map(|capsule| jka_assets::source1_jiggle::SpringCollider {
                a: capsule.a,
                b: capsule.b,
                basis: capsule.basis.unwrap_or([
                    [capsule.radius, 0.0, 0.0],
                    [0.0, capsule.radius, 0.0],
                    [0.0, 0.0, capsule.radius],
                ]),
            })
            .collect()
    }

    pub(in crate::cgame::player_presenter) fn cloth_surface_name<'a>(
        glm: &'a GlmModel,
        surface: &GlmSurface,
    ) -> Option<&'a str> {
        glm.hierarchy
            .get(surface.surface_index)
            .map(|entry| entry.name.as_str())
    }

    pub(in crate::cgame::player_presenter) fn cloth_body_capsules(
        &mut self,
        model_label: &str,
        glm: &GlmModel,
        surface_assets: &[PlayerSurfaceAsset],
        lod_index: usize,
        gla: &GlaAnimation,
        pose: &[Matrix3x4],
    ) -> Vec<ClothCapsule> {
        // Simulated hair is not part of the body: left in, its scalp vertices
        // grow the cranium proxy to the hair volume, and the bind-pose overlap
        // allowance then lets the hair sink into the head.
        let hair_cloth = self.hair_cloth_active();
        let visible = surface_assets
            .iter()
            .map(|asset| asset.surface_index)
            .filter(|&index| {
                !hair_cloth
                    || !glm.hierarchy.get(index).is_some_and(|entry| {
                        ClothSystem::is_hair_surface_name(&entry.name)
                    })
            })
            .collect::<Vec<_>>();
        let key = (model_label.to_owned(), lod_index, visible);
        let templates = self
            .cloth_body_templates
            .entry(key.clone())
            .or_insert_with(|| {
                crate::cgame::cloth_body::build_body_templates(glm, gla, &key.2, lod_index)
            });
        crate::cgame::cloth_body::pose_body_capsules(templates, gla, pose)
    }
    /// Garment surface, or a `hair*` surface while `hair_cloth` (see
    /// `hair_cloth_active`) is on.
    pub(in crate::cgame::player_presenter) fn is_cloth_surface(
        glm: &GlmModel,
        surface: &GlmSurface,
        hair_cloth: bool,
    ) -> bool {
        Self::cloth_surface_name(glm, surface).is_some_and(|name| {
            ClothSystem::is_cloth_surface_name(name)
                || (hair_cloth && ClothSystem::is_hair_surface_name(name))
        })
    }

    pub(in crate::cgame::player_presenter) fn cloth_surface_frame(
        glm: &GlmModel,
        surface: &GlmSurface,
        skinned: &Ghoul2SkinnedSurface,
        pose: &[Matrix3x4],
        hair_cloth: bool,
    ) -> Option<ClothSurfaceFrame> {
        if !Self::is_cloth_surface(glm, surface, hair_cloth) {
            return None;
        }
        let surface_name = Self::cloth_surface_name(glm, surface)?;
        // These transforms let the garment solver derive attachment motion
        // from the actual skin weights. No bone names or animation IDs enter
        // the rule for how hanging fabric follows those attachments.
        let skin_transforms = surface
            .vertices
            .iter()
            .map(|vertex| {
                let mut matrix = [[0.0; 4]; 3];
                for weight in &vertex.weights {
                    let index = *surface.bone_references.get(weight.local_bone_index)?;
                    let bone = pose.get(index)?;
                    for row in 0..3 {
                        for column in 0..4 {
                            matrix[row][column] += bone[row][column] * weight.weight;
                        }
                    }
                }
                Some(matrix)
            })
            .collect::<Option<Vec<_>>>()?;
        Some(ClothSurfaceFrame {
            surface_index: surface.surface_index,
            surface_name: surface_name.to_owned(),
            bind_positions: surface
                .vertices
                .iter()
                .map(|vertex| vertex.position)
                .collect(),
            posed_positions: skinned
                .vertices
                .iter()
                .map(|vertex| vertex.position)
                .collect(),
            posed_normals: skinned
                .vertices
                .iter()
                .map(|vertex| vertex.normal)
                .collect(),
            skin_transforms,
            triangles: surface.triangles.clone(),
        })
    }

    pub(in crate::cgame::player_presenter) fn simulate_cloth_frames(
        &mut self,
        entity_num: u16,
        model_label: &str,
        lod_index: usize,
        frames: Vec<ClothSurfaceFrame>,
        glm: &GlmModel,
        surface_assets: &[PlayerSurfaceAsset],
        gla: &GlaAnimation,
        pose: &[Matrix3x4],
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
    ) -> HashMap<usize, ClothOutput> {
        if !self.cloth.enabled() || frames.is_empty() {
            return HashMap::new();
        }
        let capsules =
            self.cloth_body_capsules(model_label, glm, surface_assets, lod_index, gla, pose);
        let wind = self
            .cloth_weather_wind
            .map(|wind| {
                let velocity = wind.at(self.stage_time_ms as f32 * 0.001);
                // Shared weather uses render X/Z; cloth uses JKA X/Y/Z.
                scene::jka_position([velocity[0], 0.0, velocity[1]])
            })
            .unwrap_or([0.0; 3]);
        self.cloth.set_wind_velocity(wind);
        self.cloth
            .simulate_garment(
                entity_num,
                model_label,
                lod_index,
                frames,
                capsules,
                ClothMotion { axis, origin },
                self.stage_time_ms,
            )
            .unwrap_or_default()
    }

    /// This frame's GPU cloth inputs for one player, or `None` while the
    /// static garment is still building (the stock surfaces draw meanwhile).
    #[allow(clippy::too_many_arguments)]
    pub(in crate::cgame::player_presenter) fn gpu_cloth_frame(
        &mut self,
        entity_num: u16,
        model_label: &str,
        lod_index: usize,
        jobs: &[(&PlayerSurfaceAsset, &GlmSurface, &Arc<Ghoul2GpuMeshSource>)],
        glm: &GlmModel,
        surface_assets: &[PlayerSurfaceAsset],
        gla: &GlaAnimation,
        pose: &[Matrix3x4],
        bones: &Arc<Vec<crate::renderer::Ghoul2GpuBone>>,
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        tuning: crate::renderer::ClothGpuTuning,
    ) -> Option<(Arc<crate::renderer::ClothGpuFrame>, HashMap<usize, ClothGpuDraw>)> {
        let mut surfaces = Vec::new();
        for &(_, surface, gpu_mesh) in jobs {
            if !Self::cloth_surface_name(glm, surface).is_some_and(ClothSystem::is_cloth_surface_name) {
                continue;
            }
            let remesh = self.cloth_remesh(surface, gpu_mesh);
            surfaces.push((surface, gpu_mesh, remesh));
        }
        if surfaces.is_empty() {
            return None;
        }
        let key: Arc<str> = Arc::from(format!(
            "{model_label}#lod{lod_index}#{}#cloth_gpu_v1",
            surfaces
                .iter()
                .map(|(surface, _, _)| surface.surface_index.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ));
        let garment = match self.cloth_gpu_garments.get(&key) {
            Some(ClothGpuGarmentSlot::Ready(garment)) => Arc::clone(garment),
            Some(ClothGpuGarmentSlot::Failed) => return None,
            Some(ClothGpuGarmentSlot::Pending(rx)) => match rx.try_recv() {
                Ok(Ok(garment)) => {
                    let garment = Arc::new(garment);
                    self.cloth_gpu_garments
                        .insert(Arc::clone(&key), ClothGpuGarmentSlot::Ready(Arc::clone(&garment)));
                    garment
                }
                Ok(Err(error)) => {
                    devprintln!(1, "CLOTH GPU: {model_label} garment rejected: {error}");
                    self.cloth_gpu_garments.insert(key, ClothGpuGarmentSlot::Failed);
                    return None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.cloth_gpu_garments.insert(key, ClothGpuGarmentSlot::Failed);
                    return None;
                }
            },
            None => {
                // One CPU skin at the current pose seeds the stitched garment,
                // exactly as the CPU solver's first frame does.
                let mut frames = Vec::with_capacity(surfaces.len());
                let mut vertices = Vec::with_capacity(surfaces.len());
                for (surface, _, remesh) in &surfaces {
                    let source: &GlmSurface = remesh.as_deref().map_or(*surface, |r| &r.surface);
                    let frame = crate::cgame::player_presenter::skin_surface_timed(
                        &mut self.perf,
                        source,
                        pose,
                    )
                    .ok()
                    .and_then(|skinned| Self::cloth_surface_frame(glm, source, &skinned, pose, false));
                    let gpu_vertices = (0..source.vertices.len())
                        .map(|index| {
                            crate::cgame::player_presenter::submission::mesh::gpu_vertex_from_glm(
                                source, index, None, lod_index,
                            )
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .ok();
                    let (Some(frame), Some(gpu_vertices)) = (frame, gpu_vertices) else {
                        self.cloth_gpu_garments.insert(key, ClothGpuGarmentSlot::Failed);
                        return None;
                    };
                    frames.push(frame);
                    vertices.push(gpu_vertices);
                }
                let capsules =
                    self.cloth_body_capsules(model_label, glm, surface_assets, lod_index, gla, pose);
                let config = self.cloth.config();
                let (tx, rx) = std::sync::mpsc::channel();
                let build_key = Arc::clone(&key);
                let label = model_label.to_owned();
                let spawned = std::thread::Builder::new()
                    .name("jka-cloth-gpu-build".into())
                    .spawn(move || {
                        let _ = tx.send(crate::cgame::cloth_gpu::build_garment(
                            build_key, &label, lod_index, &frames, vertices, &capsules, config,
                        ));
                    });
                self.cloth_gpu_garments.insert(
                    key,
                    if spawned.is_ok() {
                        ClothGpuGarmentSlot::Pending(rx)
                    } else {
                        ClothGpuGarmentSlot::Failed
                    },
                );
                return None;
            }
        };
        if garment.surface_ranges.len() != surfaces.len() {
            return None;
        }

        let config = self.cloth.config();
        let colliders = if config.body_collision {
            let capsules =
                self.cloth_body_capsules(model_label, glm, surface_assets, lod_index, gla, pose);
            crate::cgame::cloth_gpu::pack_colliders(&capsules, config.body_clearance.clamp(0.0, 4.0))
        } else {
            Vec::new()
        };
        // Reference `wind` export: shared weather wind, scaled by r_clothAirResistance.
        let wind = self
            .cloth_weather_wind
            .map(|wind| {
                let velocity = wind.at(self.stage_time_ms as f32 * 0.001);
                scene::jka_position([velocity[0], 0.0, velocity[1]])
            })
            .unwrap_or([0.0; 3])
            .map(|component| component * 0.0254 * config.air_resistance);
        let frame = Arc::new(crate::renderer::ClothGpuFrame {
            entity_num,
            garment,
            bones: Arc::clone(bones),
            colliders,
            axis,
            origin,
            time_ms: self.stage_time_ms,
            wind,
            tuning,
        });
        let draws = surfaces
            .into_iter()
            .enumerate()
            .map(|(ordinal, (surface, gpu_mesh, remesh))| {
                (
                    surface.surface_index,
                    ClothGpuDraw {
                        ordinal: ordinal as u32,
                        indices: Arc::clone(
                            remesh.as_ref().map_or(&gpu_mesh.cpu_indices, |r| &r.indices),
                        ),
                        key: Arc::clone(remesh.as_ref().map_or(&gpu_mesh.base_key, |r| &r.key)),
                    },
                )
            })
            .collect();
        Some((frame, draws))
    }

    pub(in crate::cgame::player_presenter) fn cpu_surface_vertices_with_cloth(
        skinned: Ghoul2SkinnedSurface,
        cloth_output: Option<ClothOutput>,
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        color: [f32; 4],
    ) -> Vec<DynamicModelVertex> {
        if let Some(output) = cloth_output {
            if output.positions.len() == skinned.vertices.len()
                && output.normals.len() == skinned.vertices.len()
            {
                return skinned
                    .vertices
                    .into_iter()
                    .zip(output.positions)
                    .zip(output.normals)
                    .map(|((vertex, position), normal)| DynamicModelVertex { foliage_wind: 0.0,
                        position: transform_model_point(position, axis, origin),
                        normal: transform_model_normal(normal, axis),
                        uv: vertex.uv,
                        color,
                        depth_hack: 0.0,
                        water_wetness: 0.0,
                    })
                    .collect();
            }
        }

        skinned
            .vertices
            .into_iter()
            .map(|vertex| DynamicModelVertex { foliage_wind: 0.0,
                position: transform_model_point(vertex.position, axis, origin),
                normal: transform_model_normal(vertex.normal, axis),
                uv: vertex.uv,
                color,
                depth_hack: 0.0,
                water_wetness: 0.0,
            })
            .collect()
    }
}

//! Cloth.
use crate::cgame::player_presenter::{
    scene, transform_model_normal, transform_model_point, ClothCapsule, ClothMotion, ClothOutput,
    ClothSurfaceFrame, ClothSystem, DynamicModelVertex, Ghoul2SkinnedSurface, GlaAnimation,
    GlmModel, GlmSurface, HashMap, Matrix3x4, PlayerPresenter, PlayerSurfaceAsset,
};

impl PlayerPresenter {
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
        let visible = surface_assets
            .iter()
            .map(|asset| asset.surface_index)
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
    pub(in crate::cgame::player_presenter) fn cloth_surface_frame(
        glm: &GlmModel,
        surface: &GlmSurface,
        skinned: &Ghoul2SkinnedSurface,
        pose: &[Matrix3x4],
    ) -> Option<ClothSurfaceFrame> {
        let surface_name = Self::cloth_surface_name(glm, surface)?;
        if !ClothSystem::is_cloth_surface_name(surface_name) {
            return None;
        }
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
        frames: &[ClothSurfaceFrame],
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
                &capsules,
                ClothMotion { axis, origin },
                self.stage_time_ms,
            )
            .unwrap_or_default()
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
                    .map(|((vertex, position), normal)| DynamicModelVertex {
                        position: transform_model_point(position, axis, origin),
                        normal: transform_model_normal(normal, axis),
                        uv: vertex.uv,
                        color,
                        depth_hack: 0.0,
                    })
                    .collect();
            }
        }

        skinned
            .vertices
            .into_iter()
            .map(|vertex| DynamicModelVertex {
                position: transform_model_point(vertex.position, axis, origin),
                normal: transform_model_normal(vertex.normal, axis),
                uv: vertex.uv,
                color,
                depth_hack: 0.0,
            })
            .collect()
    }
}

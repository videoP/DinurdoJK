//! Diagnostics inspector.
use crate::renderer::{
    barycentric_coordinates, legacy_dlight_diagnostic, legacy_dlight_surface_mask, ray_hits_aabb,
    scene, vertex_dlight_cpu, BlendMode, Camera, DrawClass, DynamicLightsMode, DynamicModelSurface,
    DynamicShadowsMode, DynamicWireframeClass, InspectorEntityHint, Mat4, PathBuf, Renderer,
    SurfaceInspectorInfo, SurfaceInspectorSection, TcGen, TextureData, Vec2, Vec3, Vec4,
    WorldShaderFamily, PBR_PROFILE_MATERIALS, PBR_PROFILE_PARALLAX_OCCLUSION,
    RT_SUN_ANGULAR_DIAMETER_RADIANS, SHADOW_CASCADES,
};

#[derive(Debug, Clone, Copy)]
pub(in crate::renderer) struct InspectorVertex {
    pub(in crate::renderer) position: [f32; 3],
    pub(in crate::renderer) normal: [f32; 3],
    pub(in crate::renderer) uv: [f32; 2],
    pub(in crate::renderer) lightmap_uv: [f32; 2],
}

#[derive(Debug, Clone)]
pub(in crate::renderer) struct InspectorTextureMeta {
    pub(in crate::renderer) label: String,
    pub(in crate::renderer) source: Option<PathBuf>,
    pub(in crate::renderer) width: u32,
    pub(in crate::renderer) height: u32,
    pub(in crate::renderer) mip_level_count: u32,
    pub(in crate::renderer) clamp: bool,
    pub(in crate::renderer) srgb: bool,
    pub(in crate::renderer) sampled_rgb_min: [u8; 3],
    pub(in crate::renderer) sampled_rgb_max: [u8; 3],
    pub(in crate::renderer) sampled_luma_mean: f32,
    pub(in crate::renderer) sampled_luma_stddev: f32,
}

pub(in crate::renderer) fn inspector_texture_meta(texture: &TextureData) -> InspectorTextureMeta {
    // Keep this cheap even on huge texture packs: sample at most ~4096 base-level
    // pixels. This lets Surface Inspector distinguish a genuinely flat source image
    // from a textured image that is being flattened later by UV/mip sampling.
    let pixel_count = (texture.width as usize)
        .saturating_mul(texture.height as usize)
        .min(texture.rgba.len() / 4);
    let step = pixel_count.div_ceil(4096).max(1);
    let mut minimum = [u8::MAX; 3];
    let mut maximum = [u8::MIN; 3];
    let mut luma_sum = 0.0_f64;
    let mut luma_sq_sum = 0.0_f64;
    let mut samples = 0_u64;
    for pixel in (0..pixel_count).step_by(step) {
        let offset = pixel * 4;
        let rgb = [
            texture.rgba[offset],
            texture.rgba[offset + 1],
            texture.rgba[offset + 2],
        ];
        for channel in 0..3 {
            minimum[channel] = minimum[channel].min(rgb[channel]);
            maximum[channel] = maximum[channel].max(rgb[channel]);
        }
        let luma =
            0.2126 * f64::from(rgb[0]) + 0.7152 * f64::from(rgb[1]) + 0.0722 * f64::from(rgb[2]);
        luma_sum += luma;
        luma_sq_sum += luma * luma;
        samples += 1;
    }
    let (mean, stddev) = if samples == 0 {
        (0.0, 0.0)
    } else {
        let mean = luma_sum / samples as f64;
        let variance = (luma_sq_sum / samples as f64 - mean * mean).max(0.0);
        (mean as f32, variance.sqrt() as f32)
    };
    InspectorTextureMeta {
        label: texture.label.clone(),
        source: texture.source.clone(),
        width: texture.width,
        height: texture.height,
        mip_level_count: texture.mip_level_count,
        clamp: texture.clamp,
        srgb: texture.srgb,
        sampled_rgb_min: minimum,
        sampled_rgb_max: maximum,
        sampled_luma_mean: mean,
        sampled_luma_stddev: stddev,
    }
}

impl Renderer {
    pub(in crate::renderer) fn inspect_surface(
        &mut self,
        camera: &Camera,
        dynamic_models: &[DynamicModelSurface],
        x: f32,
        y: f32,
        width: u32,
        height: u32,
        entity_hint: Option<InspectorEntityHint>,
    ) -> Option<SurfaceInspectorInfo> {
        let Some(world) = &self.world else {
            self.inspector_vertex_range = None;
            self.inspector_entity_num = None;
            return None;
        };
        if width == 0 || height == 0 {
            self.inspector_vertex_range = None;
            self.inspector_entity_num = None;
            return None;
        }

        let uv = Vec2::new(
            (x / width as f32).clamp(0.0, 1.0),
            (y / height as f32).clamp(0.0, 1.0),
        );
        let ndc = Vec4::new(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 1.0, 1.0);
        let inv_view_proj = camera.view_projection(width, height).inverse();
        let world_near = inv_view_proj * ndc;
        if world_near.w.abs() <= 1e-6 {
            self.inspector_vertex_range = None;
            self.inspector_entity_num = None;
            return None;
        }
        let ray_point = world_near.truncate() / world_near.w;
        let ray_origin = camera.position;
        let ray_direction = (ray_point - ray_origin).normalize_or_zero();
        if ray_direction.length_squared() <= 1e-8 {
            self.inspector_vertex_range = None;
            self.inspector_entity_num = None;
            return None;
        }

        let mut best: Option<(f32, usize, u32, Vec3)> = None;
        for (batch_index, batch) in world.coarse_batches.iter().enumerate() {
            if !ray_hits_aabb(
                ray_origin,
                ray_direction,
                Vec3::from_array(batch.bounds_min),
                Vec3::from_array(batch.bounds_max),
            ) {
                continue;
            }
            let start = usize::try_from(batch.source.vertices.start).unwrap_or(usize::MAX);
            let end = usize::try_from(batch.source.vertices.end)
                .unwrap_or(usize::MAX)
                .min(world.inspector_vertices.len());
            if start >= end || end - start < 3 {
                continue;
            }
            for (triangle_index, triangle) in world.inspector_vertices[start..end]
                .chunks_exact(3)
                .enumerate()
            {
                let a = Vec3::from_array(triangle[0].position);
                let b = Vec3::from_array(triangle[1].position);
                let c = Vec3::from_array(triangle[2].position);
                let Some(distance) = ray_triangle_distance(ray_origin, ray_direction, a, b, c)
                else {
                    continue;
                };
                let replace = match best {
                    None => true,
                    Some((best_distance, best_index, _, _)) => {
                        distance + 0.001 < best_distance
                            || ((distance - best_distance).abs() <= 0.001
                                && batch.source.texture.is_some()
                                && world.coarse_batches[best_index].source.texture.is_none())
                    }
                };
                if replace {
                    best = Some((
                        distance,
                        batch_index,
                        batch.source.vertices.start + (triangle_index as u32) * 3,
                        ray_origin + ray_direction * distance,
                    ));
                }
            }
        }

        // Dynamic entity geometry is not part of the BSP CPU inspector copy.
        // Trace the final CPU-presented MD3/GLM triangles too so `trace` can
        // identify visible entities and their material instead of always
        // falling through to the wall behind them. GPU-skinned Ghoul2 surfaces
        // are intentionally not guessed from bind-pose data; the CGame collision
        // hint below still identifies those entities.
        let mut dynamic_best: Option<(f32, usize, usize, Vec3)> = None;
        for (surface_index, surface) in dynamic_models.iter().enumerate() {
            if !surface.raster_visible || surface.wireframe_class == DynamicWireframeClass::Effect {
                continue;
            }
            if surface.ghoul2_gpu.is_some() || surface.fx_gpu_sprites.is_some() {
                continue;
            }
            for (triangle_index, triangle) in surface.indices.chunks_exact(3).enumerate() {
                let Some(a) = surface.vertices.get(triangle[0] as usize) else {
                    continue;
                };
                let Some(b) = surface.vertices.get(triangle[1] as usize) else {
                    continue;
                };
                let Some(c) = surface.vertices.get(triangle[2] as usize) else {
                    continue;
                };
                let Some(distance) = ray_triangle_distance(
                    ray_origin,
                    ray_direction,
                    Vec3::from_array(a.position),
                    Vec3::from_array(b.position),
                    Vec3::from_array(c.position),
                ) else {
                    continue;
                };
                if dynamic_best.is_none_or(|(best_distance, _, _, _)| distance < best_distance) {
                    dynamic_best = Some((
                        distance,
                        surface_index,
                        triangle_index,
                        ray_origin + ray_direction * distance,
                    ));
                }
            }
        }

        let world_distance = best.map_or(f32::INFINITY, |entry| entry.0);
        let dynamic_distance = dynamic_best.map_or(f32::INFINITY, |entry| entry.0);

        // The OpenJK-style CGame collision trace supplies protocol entity ownership
        // for solid hits. This catches inline bmodels and GPU-skinned players even
        // when their final raster triangles are unavailable to this CPU path.
        let hinted_entity = entity_hint.filter(|hint| {
            hint.distance <= world_distance + 0.5 && hint.distance <= dynamic_distance + 8.0
        });

        let dynamic_wins = dynamic_best.is_some_and(|(distance, surface_index, _, _)| {
            let entity_matches_hint = entity_hint.is_none_or(|hint| {
                dynamic_models
                    .get(surface_index)
                    .is_some_and(|surface| surface.entity_num == hint.entity_num)
            });
            entity_matches_hint && distance <= world_distance + 0.001
        });

        if dynamic_wins {
            let (distance, surface_index, triangle_index, hit) = dynamic_best.unwrap();
            let surface = &dynamic_models[surface_index];
            if self.inspector_entity_num == Some(surface.entity_num)
                && self.inspector_vertex_range.is_none()
            {
                self.inspector_entity_num = None;
                return None;
            }
            self.inspector_vertex_range = None;
            self.inspector_entity_num = Some(surface.entity_num);
            let texture = surface.texture.as_ref();
            let texture_name =
                texture.map_or("<untextured / generated>", |texture| texture.label.as_str());
            let entity_kind = match surface.wireframe_class {
                DynamicWireframeClass::Player => "PLAYER MODEL",
                DynamicWireframeClass::Entity => "ENTITY MODEL",
                DynamicWireframeClass::Effect => "EFFECT GEOMETRY",
            };
            let mut lines = vec![
                format!(
                    "ENTITY: #{}    GEOMETRY: {}",
                    surface.entity_num, entity_kind
                ),
                format!(
                    "DISTANCE: {distance:.2}    HIT: {:.1}, {:.1}, {:.1}",
                    hit.x, hit.y, hit.z
                ),
                format!("TRIANGLE: {triangle_index}    SURFACE SLOT: {surface_index}"),
                format!(
                    "VERTICES: {}    INDICES: {}    TRIANGLES: {}",
                    surface.vertex_count(),
                    surface.index_count(),
                    surface.index_count() / 3
                ),
                format!(
                    "ALPHA MODE: {:?}    WIREFRAME CLASS: {:?}",
                    surface.alpha_mode, surface.wireframe_class
                ),
                format!("TEXTURE: {texture_name}"),
            ];
            if let Some(texture) = texture {
                lines.push(format!(
                    "TEXTURE DETAIL: {}x{}    MIPS {}    {} {}",
                    texture.width,
                    texture.height,
                    texture.mip_level_count,
                    if texture.srgb { "sRGB" } else { "linear" },
                    if texture.clamp { "clamp" } else { "repeat" },
                ));
                lines.push(format!(
                    "IMAGE PROVIDER: {}",
                    texture.source.as_deref().map_or_else(
                        || "<generated / unknown>".to_owned(),
                        |path| path.display().to_string(),
                    )
                ));
            }
            return Some(SurfaceInspectorInfo {
                kind: entity_kind.into(),
                title: format!("ENTITY #{}", surface.entity_num),
                summary: vec![
                    (
                        "ENTITY".into(),
                        format!("#{} · {}", surface.entity_num, entity_kind),
                    ),
                    ("DISTANCE".into(), format!("{distance:.2} u")),
                    ("MATERIAL".into(), texture_name.to_owned()),
                    (
                        "GEOMETRY".into(),
                        format!(
                            "{} tris · {} verts",
                            surface.index_count() / 3,
                            surface.vertex_count()
                        ),
                    ),
                ],
                sections: vec![SurfaceInspectorSection {
                    title: "RENDER GEOMETRY".into(),
                    lines: vec![
                        format!(
                            "Hit triangle {triangle_index} at {:.1}, {:.1}, {:.1}",
                            hit.x, hit.y, hit.z
                        ),
                        format!("Blend/alpha: {:?}", surface.alpha_mode),
                        format!("Texture: {texture_name}"),
                    ],
                }],
                lines,
                hit_entity_num: (surface.entity_num < 1022).then_some(surface.entity_num),
                hit_inline_model: None,
            });
        }

        if let Some(hint) = hinted_entity {
            if self.inspector_entity_num == Some(hint.entity_num)
                && self.inspector_vertex_range.is_none()
            {
                self.inspector_entity_num = None;
                return None;
            }
            self.inspector_vertex_range = None;
            self.inspector_entity_num = Some(hint.entity_num);
            let hit = Vec3::from_array(hint.hit);
            let lines = vec![
                format!("ENTITY: #{}", hint.entity_num),
                format!(
                    "DISTANCE: {:.2}    HIT: {:.1}, {:.1}, {:.1}",
                    hint.distance, hit.x, hit.y, hit.z
                ),
                "GEOMETRY: protocol collision trace (exact bmodel or encoded entity bounds)".into(),
            ];
            return Some(SurfaceInspectorInfo {
                kind: "ENTITY".into(),
                title: format!("ENTITY #{}", hint.entity_num),
                summary: vec![
                    ("ENTITY".into(), format!("#{}", hint.entity_num)),
                    ("DISTANCE".into(), format!("{:.2} u", hint.distance)),
                    ("HIT".into(), format!("{:.1}, {:.1}, {:.1}", hit.x, hit.y, hit.z)),
                ],
                sections: vec![SurfaceInspectorSection {
                    title: "TRACE".into(),
                    lines: vec!["OpenJK-style CGame collision hit; entity state is resolved on the game thread.".into()],
                }],
                lines,
                hit_entity_num: Some(hint.entity_num),
                hit_inline_model: None,
            });
        }

        let Some((distance, batch_index, triangle_start, hit)) = best else {
            self.inspector_vertex_range = None;
            self.inspector_entity_num = None;
            return None;
        };
        self.inspector_entity_num = None;
        let hit_range = triangle_start..triangle_start.saturating_add(3);
        if self.inspector_vertex_range.as_ref() == Some(&hit_range) {
            // `trace` is a selection toggle: tracing the already-selected BSP
            // triangle exits inspection and removes its highlight.
            self.inspector_vertex_range = None;
            self.inspector_entity_num = None;
            return None;
        }
        self.inspector_vertex_range = Some(hit_range);
        let batch = &world.coarse_batches[batch_index];
        let source = &batch.source;

        let triangle_offset = usize::try_from(triangle_start).unwrap_or(usize::MAX);
        let hit_triangle = triangle_offset
            .checked_add(3)
            .filter(|end| *end <= world.inspector_vertices.len())
            .map(|end| &world.inspector_vertices[triangle_offset..end]);
        let hit_barycentric = hit_triangle.and_then(|triangle| {
            barycentric_coordinates(
                hit,
                Vec3::from_array(triangle[0].position),
                Vec3::from_array(triangle[1].position),
                Vec3::from_array(triangle[2].position),
            )
        });
        let hit_base_uv = hit_triangle
            .zip(hit_barycentric)
            .map(|(triangle, weights)| interpolate_uv(triangle, weights, |vertex| vertex.uv));
        let hit_lightmap_uv = hit_triangle
            .zip(hit_barycentric)
            .map(|(triangle, weights)| {
                interpolate_uv(triangle, weights, |vertex| vertex.lightmap_uv)
            });

        let texture = source
            .texture
            .and_then(|index| world.inspector_textures.get(index));
        let lightmap = source
            .lightmap
            .and_then(|index| world.inspector_lightmaps.get(index));
        let texture_label = texture.map(|texture| texture.label.as_str());
        let has_bsp_shader = source.bsp_shader_index != u32::MAX;
        let debug_entry = if has_bsp_shader {
            // BSP draws carry the exact material-debug entry resolved from the
            // BSP shader-table index at map load. Never infer a BSP shader from
            // its stage texture: many unrelated shaders legitimately reference
            // the same image (for example terrain_0 -> yavin/ground.jpg).
            (source.material_debug_index != u32::MAX)
                .then_some(source.material_debug_index as usize)
                .and_then(|index| world.material_debug.entries.get(index))
        } else {
            // Source .map rendering has no BSP shader-table index, so retain the
            // legacy best-effort lookup for that developer-only path.
            texture_label.and_then(|label| {
                let label_lower = label.to_ascii_lowercase();
                world.material_debug.entries.iter().find(|entry| {
                    entry.name.eq_ignore_ascii_case(label)
                        || entry
                            .lines
                            .iter()
                            .any(|line| line.to_ascii_lowercase().contains(&label_lower))
                })
            })
        };
        let title = debug_entry
            .map(|entry| entry.name.clone())
            .or_else(|| has_bsp_shader.then(|| format!("BSP SHADER #{}", source.bsp_shader_index)))
            .or_else(|| texture.map(|texture| texture.label.clone()))
            .unwrap_or_else(|| format!("WORLD BATCH {batch_index}"));

        let current_cluster = world
            .visibility
            .as_ref()
            .and_then(|vis| vis.cluster_at(scene::jka_position(camera.position.to_array())));
        let pvs_visible = current_cluster.is_none_or(|cluster| {
            if source.pvs_signature.is_empty() {
                return true;
            }
            let word = cluster / 64;
            let bit = cluster % 64;
            source
                .pvs_signature
                .get(word)
                .is_none_or(|value| *value & (1_u64 << bit) != 0)
        });

        let vertex_count = source.vertices.end.saturating_sub(source.vertices.start);
        let mut lines = vec![
            format!(
                "BATCH: {batch_index}    TRIANGLES: {}    VERTICES: {vertex_count}",
                vertex_count / 3
            ),
            format!(
                "DISTANCE: {distance:.2}    HIT: {:.1}, {:.1}, {:.1}",
                hit.x, hit.y, hit.z
            ),
            format!(
                "CLASS: {:?}    BLEND: {:?}    CULL: {:?}",
                source.pipeline.class, source.pipeline.blend, source.pipeline.cull
            ),
            format!(
                "DEPTH WRITE: {}    DEPTH EQUAL: {}    POLYGON OFFSET: {}",
                source.pipeline.depth_write, source.pipeline.depth_equal, source.pipeline.offset
            ),
            format!(
                "TCGEN: {:?}    TCMODS: {}    RGBGEN: {:?}    ALPHAGEN: {:?}",
                source.tc_gen,
                source.tc_mods.len(),
                source.rgb_gen,
                source.alpha_gen
            ),
            format!(
                "RENDER FLAGS: MODULATE LIGHTMAP: {}    TEXTURE IS LIGHTMAP: {}    TEXTURE IS WHITE: {}",
                source.modulate_lightmap, source.texture_is_lightmap, source.texture_is_white
            ),
        ];
        if has_bsp_shader {
            let shader_index = source.bsp_shader_index;
            lines.insert(
                1,
                format!(
                    "BSP SHADER: #{shader_index}    {}",
                    debug_entry
                        .map(|entry| entry.name.as_str())
                        .unwrap_or("<unresolved>")
                ),
            );
        }
        if let Some(uv) = hit_base_uv {
            lines.push(format!("BASE UV HIT: {:.5}, {:.5}", uv.x, uv.y));
        }
        if let Some(uv) = hit_lightmap_uv {
            lines.push(format!("LIGHTMAP UV HIT: {:.5}, {:.5}", uv.x, uv.y));
        }
        if let Some(triangle) = hit_triangle {
            let base_edges = uv_edge_lengths(triangle, |vertex| vertex.uv);
            let lm_edges = uv_edge_lengths(triangle, |vertex| vertex.lightmap_uv);
            lines.push(format!(
                "BASE UV EDGE DELTAS: {:.5}, {:.5}, {:.5}",
                base_edges[0], base_edges[1], base_edges[2]
            ));
            lines.push(format!(
                "LIGHTMAP UV EDGE DELTAS: {:.5}, {:.5}, {:.5}",
                lm_edges[0], lm_edges[1], lm_edges[2]
            ));
        }
        if let Some(texture) = texture {
            lines.push(format!(
                "TEXTURE: {}    {}x{}    MIPS {}    {} {}",
                texture.label,
                texture.width,
                texture.height,
                texture.mip_level_count,
                if texture.srgb { "sRGB" } else { "linear" },
                if texture.clamp { "clamp" } else { "repeat" },
            ));
            lines.push(format!(
                "IMAGE PROVIDER: {}",
                texture.source.as_deref().map_or_else(
                    || "<generated / unknown>".to_owned(),
                    |path| path.display().to_string()
                )
            ));
            lines.push(format!(
                "SOURCE DETAIL: RGB {}..{} / {}..{} / {}..{}    LUMA AVG {:.1} STDDEV {:.1}",
                texture.sampled_rgb_min[0],
                texture.sampled_rgb_max[0],
                texture.sampled_rgb_min[1],
                texture.sampled_rgb_max[1],
                texture.sampled_rgb_min[2],
                texture.sampled_rgb_max[2],
                texture.sampled_luma_mean,
                texture.sampled_luma_stddev,
            ));
            if let Some(triangle) = hit_triangle {
                let density = texture_edge_density(triangle, texture.width, texture.height);
                lines.push(format!(
                    "BASE TEXELS/WORLD UNIT: {:.3}, {:.3}, {:.3}",
                    density[0], density[1], density[2]
                ));
            }
        } else if source.texture_is_lightmap {
            lines.push("TEXTURE: $lightmap".into());
        } else {
            lines.push("TEXTURE: <white / shader-generated>".into());
        }
        if let Some(lightmap) = lightmap {
            lines.push(format!(
                "LIGHTMAP: {}    {}x{}    PAGE {:?}",
                lightmap.label, lightmap.width, lightmap.height, source.lightmap
            ));
            lines.push(format!(
                "LIGHTMAP PROVIDER: {}",
                lightmap.source.as_deref().map_or_else(
                    || "<embedded / generated>".to_owned(),
                    |path| path.display().to_string()
                )
            ));
        } else if source.vertex_lit {
            lines.push("LIGHTMAP: Vertex-lit (LIGHTMAP_BY_VERTEX / -3; no page)".into());
        } else {
            lines.push(format!("LIGHTMAP: {:?}", source.lightmap));
        }
        let fog_assignment = if source.fog[3] > 0.001 {
            if source.fog_is_global {
                "GLOBAL"
            } else {
                "LOCAL"
            }
        } else {
            "NONE"
        };
        lines.push(format!(
            "FOG: {}    RGB {:.3},{:.3},{:.3}    DEPTH {:.1}    STAGE OVERRIDE {:?}    L1 POST {}",
            fog_assignment,
            source.fog[0],
            source.fog[1],
            source.fog[2],
            source.fog[3],
            source.fog_color_override,
            if source.global_fog_post_eligible {
                "YES"
            } else {
                "NO"
            },
        ));
        let drawfog = self.weather.fog.legacy_drawfog_value();
        let fog_path = match drawfog {
            1 if source.fog[3] > 0.001
                && source.fog_is_global
                && source.global_fog_post_eligible =>
            {
                "GLOBAL DISPLAY-SPACE (SQRT TABLE)"
            }
            1 if source.fog[3] > 0.001 => "SEPARATE PASS (SQRT TABLE)",
            2 if source.fog[3] > 0.001 && source.fog_is_global => "GLOBAL EXP2 SEPARATE PASS",
            2 if source.fog[3] > 0.001 => "LOCAL SEPARATE PASS",
            _ => "NONE",
        };
        lines.push(format!(
            "FOG MODE: r_drawfog {}    PATH: {}",
            drawfog, fog_path,
        ));
        lines.push(format!(
            "PVS CLUSTER: {}    PVS VISIBLE: {}",
            current_cluster.map_or_else(|| "none".into(), |cluster| cluster.to_string()),
            if pvs_visible { "YES" } else { "NO" },
        ));
        lines.push(format!(
            "PLANAR: {}    ENV CANDIDATE: {}    REFLECTION PROBE: {:?}",
            source.planar_reflection, source.planar_environment_candidate, source.reflection_probe,
        ));
        lines.push(format!(
            "REFLECTION POLICY: quality={}    planar_mode={}    tcGen environment={}",
            self.reflection_quality.label(),
            self.planar_reflection_mode.label(),
            if self.reflection_quality.omits_environment_stages() {
                "STRIPPED (absolute OFF)"
            } else {
                "PRESERVED"
            },
        ));
        lines.push(format!(
            "BOUNDS MIN: {:.1}, {:.1}, {:.1}",
            batch.bounds_min[0], batch.bounds_min[1], batch.bounds_min[2]
        ));
        lines.push(format!(
            "BOUNDS MAX: {:.1}, {:.1}, {:.1}",
            batch.bounds_max[0], batch.bounds_max[1], batch.bounds_max[2]
        ));
        if world.source_map {
            let map_receiver = !source.texture_is_lightmap
                && !source.vertex_lit
                && source.pipeline.class != DrawClass::Sky
                && source.pipeline.blend == BlendMode::Opaque;
            let q3map_lights = world
                .dynamic_lights
                .iter()
                .filter(|light| {
                    light.falloff != scene::DynamicLightFalloff::Smooth && light.surface_lighting
                })
                .count();
            lines.push(format!(
                "MAP LIGHT SIM: toggle={} receiver={} q3map_surface_lights={} ambient={:.3},{:.3},{:.3} minlight={:.3},{:.3},{:.3}",
                if self.map_light_simulation_enabled { "ON" } else { "OFF" },
                if map_receiver { "YES" } else { "NO" },
                q3map_lights,
                world.source_map_lighting.ambient[0],
                world.source_map_lighting.ambient[1],
                world.source_map_lighting.ambient[2],
                world.source_map_lighting.minlight[0],
                world.source_map_lighting.minlight[1],
                world.source_map_lighting.minlight[2],
            ));
        }
        // Dynamic-light diagnostics live in the surface inspector rather than a
        // continuously running GPU/CPU counter. Pressing Trace/Q therefore gives
        // exact per-triangle numbers with effectively zero cost during normal play.
        if matches!(
            self.dynamic_lights_mode,
            DynamicLightsMode::Legacy
                | DynamicLightsMode::Vertex
                | DynamicLightsMode::ClusteredLite
        ) {
            let shader_key = self.world_shader_variant_key();
            let family = match shader_key.family() {
                WorldShaderFamily::Enhanced => "enhanced",
                WorldShaderFamily::Lean => "lean",
            };
            let dlight_eligible = !source.texture_is_lightmap
                && source.pipeline.class != DrawClass::Sky
                && source.pipeline.blend == BlendMode::Opaque;
            lines.push(format!(
                "DLIGHT DEBUG: mode={} family={} variant={} runtime={} transient={} active={} surface_eligible={}",
                self.dynamic_lights_mode.config_value(),
                family,
                shader_key.short_label(),
                if self.runtime_transient_lights_enabled() { "YES" } else { "NO" },
                self.transient_lights.len(),
                self.active_transient_light_count(),
                if dlight_eligible { "YES" } else { "NO" },
            ));
            lines.push(format!(
                "DLIGHT PIPELINE FLAGS: legacy={} vertex={} clustered_lite={} classic_r_vertexLight={}",
                u8::from(shader_key.legacy_dlights),
                u8::from(shader_key.vertex_dlights),
                u8::from(shader_key.clustered_lite_dlights),
                u8::from(self.classic_vertex_light),
            ));
            if self.dynamic_lights_mode == DynamicLightsMode::Legacy {
                let source_triangle = triangle_start / 3;
                if let Some(&surface_index) = world
                    .legacy_dlight_triangle_surfaces
                    .get(source_triangle as usize)
                {
                    if let Some(surface) = world.legacy_dlight_surfaces.get(surface_index as usize)
                    {
                        let transient_count = self.active_transient_light_count().min(32);
                        let mask = legacy_dlight_surface_mask(
                            surface,
                            &self.transient_lights,
                            transient_count,
                        );
                        let kind = match surface.cull_kind {
                            0 => "REJECT",
                            1 => "BOUNDS+FACE-PLANE",
                            2 => "BOUNDS",
                            _ => "UNKNOWN",
                        };
                        lines.push(format!(
                            "LEGACY OPENJK CULL: surface={} kind={} candidate_lights={}/{} mask=0x{:08x}",
                            surface_index,
                            kind,
                            mask.count_ones(),
                            transient_count,
                            mask,
                        ));
                    }
                }
            }

            let primary_light =
                self.transient_lights
                    .iter()
                    .take(32)
                    .enumerate()
                    .min_by(|(_, a), (_, b)| {
                        let da = (Vec3::from_array(a.position) - hit).length() / a.radius.max(1.0);
                        let db = (Vec3::from_array(b.position) - hit).length() / b.radius.max(1.0);
                        da.total_cmp(&db)
                    });
            if let Some((light_index, light)) = primary_light {
                let light_position = Vec3::from_array(light.position);
                let hit_distance = (light_position - hit).length();
                lines.push(format!(
                    "DLIGHT #{light_index}: POS {:.1},{:.1},{:.1} R={:.2} HIT_DIST={:.2} ({:.3}R) RGB={:.3},{:.3},{:.3} I={:.3}",
                    light.position[0], light.position[1], light.position[2],
                    light.radius, hit_distance, hit_distance / light.radius.max(1.0),
                    light.color[0], light.color[1], light.color[2], light.intensity,
                ));

                if let Some(triangle) = hit_triangle {
                    match self.dynamic_lights_mode {
                        DynamicLightsMode::Vertex => {
                            let samples = [
                                vertex_dlight_cpu(
                                    Vec3::from_array(triangle[0].position),
                                    Vec3::from_array(triangle[0].normal),
                                    &self.transient_lights,
                                ),
                                vertex_dlight_cpu(
                                    Vec3::from_array(triangle[1].position),
                                    Vec3::from_array(triangle[1].normal),
                                    &self.transient_lights,
                                ),
                                vertex_dlight_cpu(
                                    Vec3::from_array(triangle[2].position),
                                    Vec3::from_array(triangle[2].normal),
                                    &self.transient_lights,
                                ),
                            ];
                            let weights = hit_barycentric.unwrap_or([1.0 / 3.0; 3]);
                            let interpolated = samples[0] * weights[0]
                                + samples[1] * weights[1]
                                + samples[2] * weights[2];
                            let vertex_distances = [
                                (Vec3::from_array(triangle[0].position) - light_position).length()
                                    / light.radius.max(1.0),
                                (Vec3::from_array(triangle[1].position) - light_position).length()
                                    / light.radius.max(1.0),
                                (Vec3::from_array(triangle[2].position) - light_position).length()
                                    / light.radius.max(1.0),
                            ];
                            lines.push(format!(
                                "VERTEX DLIGHT: vertex_dist_R={:.3},{:.3},{:.3} sample_len={:.5},{:.5},{:.5} interp_len={:.5}",
                                vertex_distances[0], vertex_distances[1], vertex_distances[2],
                                samples[0].length(), samples[1].length(), samples[2].length(),
                                interpolated.length(),
                            ));
                            lines.push(format!(
                                "VERTEX INTERP RGB: {:.5},{:.5},{:.5} bary={:.3},{:.3},{:.3}",
                                interpolated.x,
                                interpolated.y,
                                interpolated.z,
                                weights[0],
                                weights[1],
                                weights[2],
                            ));
                        }
                        DynamicLightsMode::Legacy => {
                            if let Some(diag) = legacy_dlight_diagnostic(triangle, hit, light) {
                                let predicted = Vec3::from_array(light.color)
                                    * light.intensity.max(0.0)
                                    * diag.plane_modulate
                                    * diag.blob
                                    * 0.225;
                                lines.push(format!(
                                    "LEGACY DLIGHT: smooth radial cubic plane_dist={:.3} proj_R={:.3} radial_sq={:.4} plane_mod={:.4}",
                                    diag.plane_distance, diag.projected_radius, diag.radial_sq, diag.plane_modulate,
                                ));
                                lines.push(format!(
                                    "LEGACY RESPONSE: blob={:.5} gain=0.225 predicted_len={:.5}",
                                    diag.blob,
                                    predicted.length(),
                                ));
                            }
                        }
                        DynamicLightsMode::ClusteredLite => {
                            lines.push("CLUSTERED LITE: transient light transport is active; use the same hit/light line above as the source sanity check.".into());
                        }
                        _ => {}
                    }
                }
            } else {
                lines.push(
                    "DLIGHT DEBUG: no transient/runtime lights are currently submitted.".into(),
                );
            }
        }
        if let Some(entry) = debug_entry {
            if let Some(file) = entry.definition_file.as_deref() {
                lines.push(format!("SHADER FILE: {file}"));
            }
            if let Some(provider) = entry.definition_source.as_deref() {
                lines.push(format!("SHADER PROVIDER: {}", provider.display()));
            }
            lines.push(format!(
                "SHADER SOURCE: {}{}",
                entry.source,
                if entry.mtr_override {
                    " (MTR OVERRIDE)"
                } else {
                    ""
                }
            ));
            for image in &entry.image_sources {
                lines.push(image.clone());
            }
            for detail in &entry.lines {
                lines.push(detail.clone());
            }
        } else if has_bsp_shader {
            lines.push("SHADER SOURCE: <no exact BSP material-debug entry>".into());
        } else {
            lines.push("SHADER SOURCE: <no direct material-debug match>".into());
        }

        let texture_summary = texture
            .map(|texture| texture.label.clone())
            .or_else(|| source.texture_is_lightmap.then(|| "$lightmap".into()))
            .unwrap_or_else(|| "<generated / white>".into());
        let lighting_summary = if let Some(lightmap) = lightmap {
            format!("lightmap {}", lightmap.label)
        } else if source.vertex_lit {
            "vertex-lit".into()
        } else {
            format!("lightmap {:?}", source.lightmap)
        };
        let summary = vec![
            ("MATERIAL".into(), title.clone()),
            ("DISTANCE".into(), format!("{distance:.2} u")),
            (
                "DRAW".into(),
                format!(
                    "{:?} · {:?} · {:?}",
                    source.pipeline.class, source.pipeline.blend, source.pipeline.cull
                ),
            ),
            ("TEXTURE".into(), texture_summary.clone()),
            ("LIGHTING".into(), lighting_summary.clone()),
        ];
        let mut sections = vec![
            SurfaceInspectorSection {
                title: "SURFACE".into(),
                lines: vec![
                    format!(
                        "Batch {batch_index} · {} triangles · {vertex_count} vertices",
                        vertex_count / 3
                    ),
                    format!("Hit {:.1}, {:.1}, {:.1}", hit.x, hit.y, hit.z),
                    format!(
                        "Depth: write={} equal={} offset={}",
                        source.pipeline.depth_write,
                        source.pipeline.depth_equal,
                        source.pipeline.offset
                    ),
                ],
            },
            SurfaceInspectorSection {
                title: "MATERIAL / LIGHTING".into(),
                lines: vec![
                    format!("Texture: {texture_summary}"),
                    format!("Lighting: {lighting_summary}"),
                    format!(
                        "Fog: {fog_assignment} · PVS visible: {}",
                        if pvs_visible { "YES" } else { "NO" }
                    ),
                    format!(
                        "Planar: {} · env candidate: {} · probe: {:?}",
                        source.planar_reflection,
                        source.planar_environment_candidate,
                        source.reflection_probe
                    ),
                ],
            },
        ];
        if let Some(definition_text) =
            debug_entry.and_then(|entry| entry.definition_text.as_deref())
        {
            sections.insert(
                0,
                SurfaceInspectorSection {
                    title: "SHADER SCRIPT".into(),
                    lines: definition_text.lines().map(str::to_owned).collect(),
                },
            );
        }

        Some(SurfaceInspectorInfo {
            kind: "WORLD SURFACE".into(),
            title,
            summary,
            sections,
            lines,
            hit_entity_num: None,
            hit_inline_model: None,
        })
    }

    pub(in crate::renderer) fn dump_materials(&self, filter: Option<&str>, all: bool) {
        println!("\n=== JKA MATERIAL DEBUG ===");
        let Some(world) = &self.world else {
            println!("No world is loaded.");
            println!("=== END JKA MATERIAL DEBUG ===\n");
            return;
        };

        let debug = &world.material_debug;
        println!(
            "library: .shader files={} parsed_defs={} | .mtr files={} parsed_defs={}",
            debug.shader_files, debug.shader_definitions, debug.mtr_files, debug.mtr_definitions,
        );
        println!(
            "runtime: PBR={} POM={} clustered_lights={} dynamic_lights={} local_shadows={} cascaded_sun={} static_lightgrid={} hdr_lightgrid={} reflection_probes={}",
            self.pbr_enabled && PBR_PROFILE_MATERIALS,
            self.pbr_enabled && PBR_PROFILE_PARALLAX_OCCLUSION,
            self.point_lighting_enabled(),
            world.light_count,
            self.local_light_shadows_enabled,
            self.cascaded_shadows_enabled,
            world._static_light_grid.enabled,
            world._static_light_grid.external_hdr,
            world.reflection_probes.source_count,
        );
        if self.pbr_enabled
            && PBR_PROFILE_MATERIALS
            && !world._static_light_grid.enabled
            && world.reflection_probes.source_count == 0
            && (!self.point_lighting_enabled() || world.light_count == 0)
        {
            println!(
                "NOTE: PBR companions are enabled, but no static directional lightgrid, reflection probes, or active clustered lights are available to make normal/roughness/metallic/specular visibly respond."
            );
        }
        if self.pbr_enabled && PBR_PROFILE_MATERIALS && world._static_light_grid.enabled {
            println!(
                "NOTE: baked lightmaps are directionally relit from the JKA/Rend2 lightgrid for normal/specular response{}.",
                if world._static_light_grid.external_hdr {
                    " (external HDR lightgrid.raw loaded)"
                } else {
                    ""
                }
            );
        }
        if self.pbr_enabled && PBR_PROFILE_MATERIALS && world.reflection_probes.source_count > 0 {
            println!(
                "NOTE: Rend2 reflection probes are loaded; roughness/metallic/specular materials sample the nearest assigned cubemap."
            );
        }
        println!(
            "NOTE: POM is independent of clustered lighting. Emissive companions are also applied directly when PBR is ON."
        );

        let mut enhanced_batches = 0usize;
        let mut pbr_eligible_batches = 0usize;
        let mut pom_eligible_batches = 0usize;
        let mut probe_assigned_batches = 0usize;
        for batch in &world.coarse_batches {
            let source = &batch.source;
            let enhanced = source.normal_texture.is_some()
                || source.roughness_texture.is_some()
                || source.height_texture.is_some()
                || source.metallic_texture.is_some()
                || source.specular_texture.is_some()
                || source.emissive_texture.is_some()
                || source.roughness_override.is_some()
                || source.specular_reflectance.is_some();
            if !enhanced {
                continue;
            }
            enhanced_batches += 1;
            let eligible = !source.texture_is_lightmap
                && !source.texture_is_white
                && source.pipeline.class == DrawClass::Opaque
                && source.pipeline.blend == BlendMode::Opaque
                && matches!(source.tc_gen, TcGen::Base);
            if eligible {
                pbr_eligible_batches += 1;
                if source.height_texture.is_some() {
                    pom_eligible_batches += 1;
                }
                if source.reflection_probe.is_some() {
                    probe_assigned_batches += 1;
                }
            }
        }
        println!(
            "prepared coarse batches: total={} enhanced={} pbr_eligible={} pom_eligible={} reflection_probe_assigned={}",
            world.coarse_batches.len(),
            enhanced_batches,
            pbr_eligible_batches,
            pom_eligible_batches,
            probe_assigned_batches
        );

        let normalized_filter = filter.map(|value| value.to_ascii_lowercase());
        println!(
            "listing: {}{}",
            if all {
                "ALL used materials"
            } else if normalized_filter.is_some() {
                "matching materials"
            } else {
                "enhanced materials only"
            },
            normalized_filter
                .as_deref()
                .map(|filter| format!(" (filter={filter})"))
                .unwrap_or_default()
        );

        let mut shown = 0usize;
        for entry in &debug.entries {
            if !all && normalized_filter.is_none() && !entry.enhanced {
                continue;
            }
            if let Some(filter) = normalized_filter.as_deref() {
                let name_match = entry.name.to_ascii_lowercase().contains(filter);
                let source_match = entry.source.to_ascii_lowercase().contains(filter);
                let line_match = entry
                    .lines
                    .iter()
                    .any(|line| line.to_ascii_lowercase().contains(filter));
                if !name_match && !source_match && !line_match {
                    continue;
                }
            }
            shown += 1;
            println!(
                "\n[MATERIAL] {} | source={}{} | enhanced={}",
                entry.name,
                entry.source,
                if entry.mtr_override {
                    " [MTR OVERRIDE]"
                } else {
                    ""
                },
                entry.enhanced,
            );
            for line in &entry.lines {
                println!("  {line}");
            }
        }
        if shown == 0 {
            println!("No material entries matched this dump.");
        } else {
            println!("\nshown materials: {shown}/{}", debug.entries.len());
        }
        println!("=== END JKA MATERIAL DEBUG ===\n");
    }

    pub(in crate::renderer) fn set_cascaded_shadows(&mut self, mode: DynamicShadowsMode) {
        self.cascaded_shadow_mode = mode;
        self.update_lighting_settings();
        // The Entity map is receiver-compatible with the cascade path (same
        // shader variant, one cascade), so it shares the enabled flag.
        self.cascaded_shadows_enabled = matches!(
            mode,
            DynamicShadowsMode::CascadedShadowMaps | DynamicShadowsMode::EntityMap
        );
        // Re-aim on entry instead of easing from a stale light.
        self.entity_shadow.initialized = false;
        let rt_active = if mode == DynamicShadowsMode::RayTraced {
            let active = self.ensure_ray_traced_shadow_resources();
            if active {
                if let Some(rt) = self.ray_traced_shadows.as_ref() {
                    println!(
                        "RT Shadows: active - hardware ray queries, {} BLAS geometries / {} static opaque BSP triangles; soft directional emitter {:.2} deg, {} spp{}",
                        rt.geometry_count,
                        rt.triangle_count,
                        RT_SUN_ANGULAR_DIAMETER_RADIANS.to_degrees(),
                        self.rt_samples,
                        if self.taa_enabled { " + temporal TAA" } else { " (stable spatial sample)" }
                    );
                }
            } else if !self.ray_tracing_supported {
                println!("RT Shadows: requested but unsupported on the current adapter/backend");
            }
            active
        } else {
            false
        };
        self.rebuild_weather_surface_receiver_bind_group();
        self.rebuild_frame_plan();
        self.activate_world_pipeline_variant();
        if !self.cascaded_shadows_enabled && !rt_active {
            self.write_shadow_uniforms(
                &[Mat4::IDENTITY; SHADOW_CASCADES],
                &[0.0; SHADOW_CASCADES],
                &[0.0; SHADOW_CASCADES],
                false,
                Vec3::Z,
                [0.0; 4],
                false,
            );
        }
    }
}

pub(in crate::renderer) fn interpolate_uv(
    triangle: &[InspectorVertex],
    weights: [f32; 3],
    uv: impl Fn(&InspectorVertex) -> [f32; 2],
) -> Vec2 {
    let a = Vec2::from_array(uv(&triangle[0]));
    let b = Vec2::from_array(uv(&triangle[1]));
    let c = Vec2::from_array(uv(&triangle[2]));
    a * weights[0] + b * weights[1] + c * weights[2]
}

pub(in crate::renderer) fn uv_edge_lengths(
    triangle: &[InspectorVertex],
    uv: impl Fn(&InspectorVertex) -> [f32; 2],
) -> [f32; 3] {
    let a = Vec2::from_array(uv(&triangle[0]));
    let b = Vec2::from_array(uv(&triangle[1]));
    let c = Vec2::from_array(uv(&triangle[2]));
    [(b - a).length(), (c - b).length(), (a - c).length()]
}

pub(in crate::renderer) fn texture_edge_density(
    triangle: &[InspectorVertex],
    width: u32,
    height: u32,
) -> [f32; 3] {
    let positions = [
        Vec3::from_array(triangle[0].position),
        Vec3::from_array(triangle[1].position),
        Vec3::from_array(triangle[2].position),
    ];
    let uvs = [
        Vec2::from_array(triangle[0].uv),
        Vec2::from_array(triangle[1].uv),
        Vec2::from_array(triangle[2].uv),
    ];
    let size = Vec2::new(width as f32, height as f32);
    let pairs = [(0usize, 1usize), (1, 2), (2, 0)];
    pairs.map(|(a, b)| {
        let world = (positions[b] - positions[a]).length();
        if world <= 1e-6 {
            0.0
        } else {
            ((uvs[b] - uvs[a]) * size).length() / world
        }
    })
}

pub(in crate::renderer) fn ray_triangle_distance(
    origin: Vec3,
    direction: Vec3,
    a: Vec3,
    b: Vec3,
    c: Vec3,
) -> Option<f32> {
    let edge1 = b - a;
    let edge2 = c - a;
    let p = direction.cross(edge2);
    let determinant = edge1.dot(p);
    if determinant.abs() < 1e-7 {
        return None;
    }
    let inverse = 1.0 / determinant;
    let t = origin - a;
    let u = t.dot(p) * inverse;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = t.cross(edge1);
    let v = direction.dot(q) * inverse;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let distance = edge2.dot(q) * inverse;
    (distance > 0.001 && distance.is_finite()).then_some(distance)
}

//! Vegetation.
use crate::scene::{
    bsp_fog_params, materials, pvs_signature, render_position, Arc, BTreeMap, Bsp, GrassInstance,
    GrassPatch, GrassPatchKey, GrassWorkerPatchKey, HashMap, Instant, MapJobPool, Shader,
    StageFingerprint, StageTexture, SurfaceMaterial, Task, TextureData, Textures, Vec3,
    GRASS_FULL_DENSITY_SPACING, GRASS_MAX_BLADES_PER_TRIANGLE, GRASS_MAX_SPACING,
    GRASS_MIN_SPACING, GRASS_PATCH_SIZE, GRASS_REFERENCE_SPRITE_DENSITY, LIGHTMAP_BY_VERTEX,
};

pub(in crate::scene) fn grass_hash(mut value: u32) -> u32 {
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

pub(in crate::scene) fn grass_random(seed: u32, salt: u32) -> f32 {
    let value = grass_hash(seed ^ salt);
    (value as f32) / (u32::MAX as f32)
}

pub(in crate::scene) fn srgb_u8_to_linear(value: u8) -> f32 {
    let value = f32::from(value) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

pub(in crate::scene) fn sample_rgba_srgb_bilinear(
    rgba: &[u8],
    width: u32,
    height: u32,
    uv: [f32; 2],
) -> [f32; 3] {
    if width == 0 || height == 0 || rgba.len() < width as usize * height as usize * 4 {
        return [1.0; 3];
    }
    let x = uv[0].clamp(0.0, 1.0) * (width.saturating_sub(1)) as f32;
    let y = uv[1].clamp(0.0, 1.0) * (height.saturating_sub(1)) as f32;
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(width as usize - 1);
    let y1 = (y0 + 1).min(height as usize - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let sample = |sx: usize, sy: usize| {
        let offset = (sy * width as usize + sx) * 4;
        [
            srgb_u8_to_linear(rgba[offset]),
            srgb_u8_to_linear(rgba[offset + 1]),
            srgb_u8_to_linear(rgba[offset + 2]),
        ]
    };
    let a = sample(x0, y0);
    let b = sample(x1, y0);
    let c = sample(x0, y1);
    let d = sample(x1, y1);
    let mut out = [0.0; 3];
    for channel in 0..3 {
        let top = a[channel] + (b[channel] - a[channel]) * tx;
        let bottom = c[channel] + (d[channel] - c[channel]) * tx;
        out[channel] = top + (bottom - top) * ty;
    }
    out
}

pub(in crate::scene) fn sample_embedded_lightmap_bilinear(
    bytes: &[u8],
    page: usize,
    uv: [f32; 2],
) -> Option<[f32; 3]> {
    const PAGE: usize = 128;
    const BYTES_PER_PAGE: usize = PAGE * PAGE * 3;
    let page_start = page.checked_mul(BYTES_PER_PAGE)?;
    let data = bytes.get(page_start..page_start + BYTES_PER_PAGE)?;
    let x = uv[0].clamp(0.0, 1.0) * (PAGE - 1) as f32;
    let y = uv[1].clamp(0.0, 1.0) * (PAGE - 1) as f32;
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(PAGE - 1);
    let y1 = (y0 + 1).min(PAGE - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let sample = |sx: usize, sy: usize| {
        let offset = (sy * PAGE + sx) * 3;
        [
            srgb_u8_to_linear(data[offset]),
            srgb_u8_to_linear(data[offset + 1]),
            srgb_u8_to_linear(data[offset + 2]),
        ]
    };
    let a = sample(x0, y0);
    let b = sample(x1, y0);
    let c = sample(x0, y1);
    let d = sample(x1, y1);
    let mut out = [0.0; 3];
    for channel in 0..3 {
        let top = a[channel] + (b[channel] - a[channel]) * tx;
        let bottom = c[channel] + (d[channel] - c[channel]) * tx;
        out[channel] = top + (bottom - top) * ty;
    }
    Some(out)
}

pub(in crate::scene) fn grass_ground_albedo_tint(
    material: &SurfaceMaterial,
    textures: &Textures,
) -> [u8; 4] {
    // Analyze the opaque/base image once while preparing the map. Hardware sRGB
    // sampling yields linear values in the world shader, so average in linear space
    // here too. Limit the work to ~4k texels regardless of source resolution.
    let stage = material
        .stages
        .iter()
        .find(|stage| stage.blend.is_none() && matches!(stage.texture, StageTexture::Image(_)))
        .or_else(|| {
            material
                .stages
                .iter()
                .find(|stage| matches!(stage.texture, StageTexture::Image(_)))
        });
    let Some(stage) = stage else {
        return [0, 0, 0, 0];
    };
    let StageTexture::Image(index) = stage.texture else {
        return [0, 0, 0, 0];
    };
    let Some(image) = textures.images.get(index) else {
        return [0, 0, 0, 0];
    };
    let pixel_count = (image.width as usize).saturating_mul(image.height as usize);
    let base_bytes = pixel_count.saturating_mul(4).min(image.rgba.len());
    if pixel_count == 0 || base_bytes < 4 {
        return [0, 0, 0, 0];
    }
    let stride = ((pixel_count as f64 / 4096.0).sqrt().ceil() as usize).max(1);
    let mut sum = [0.0_f64; 3];
    let mut samples = 0_u64;
    for y in (0..image.height as usize).step_by(stride) {
        for x in (0..image.width as usize).step_by(stride) {
            let offset = (y * image.width as usize + x) * 4;
            if offset + 3 >= base_bytes || image.rgba[offset + 3] < 16 {
                continue;
            }
            for channel in 0..3 {
                let raw = image.rgba[offset + channel];
                let value = if image.srgb {
                    srgb_u8_to_linear(raw)
                } else {
                    f32::from(raw) / 255.0
                };
                sum[channel] += f64::from(value * stage.color[channel]);
            }
            samples += 1;
        }
    }
    if samples == 0 {
        return [0, 0, 0, 0];
    }
    let inv = 1.0 / samples as f64;
    [
        ((sum[0] * inv).clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        ((sum[1] * inv).clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        ((sum[2] * inv).clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
        255,
    ]
}

#[derive(Debug, Clone, Copy)]
pub struct SurfaceSpriteEffectTriangle {
    /// Original JKA coordinates are retained because Raven seeds/distributes
    /// effect sprites from the triangle's authored X/Y/Z values.
    pub positions_jka: [[f32; 3]; 3],
    pub normal_z: [f32; 3],
    /// OpenJK's effect path uses the blue byte of the active vertex-color style
    /// as its scalar baked-light value.
    pub light: [u8; 3],
}

#[derive(Debug, Clone)]
pub struct SurfaceSpriteEffectEmitter {
    pub texture: usize,
    pub clamp: bool,
    pub blend: Option<materials::BlendFunc>,
    pub sprite: jka_assets::shader::SurfaceSprite,
    pub triangles: Vec<SurfaceSpriteEffectTriangle>,
    /// Same camera-cluster visibility signature used by ordinary BSP batches.
    pub pvs_signature: Vec<u64>,
}

pub(in crate::scene) fn collect_surface_sprite_effect_emitters(
    bsp: &Bsp,
    mesh: &jka_assets::bsp::Mesh,
    surface_materials: &[SurfaceMaterial],
) -> Vec<SurfaceSpriteEffectEmitter> {
    let mut emitters = Vec::new();
    for batch in &mesh.batches {
        let Some(material) = surface_materials.get(batch.shader) else {
            continue;
        };
        if material.hidden || material.surface_sprite_effects.is_empty() {
            continue;
        }
        let Some(surface) = bsp.surfaces.get(batch.surface) else {
            continue;
        };
        let lightmap_slot =
            (0..4).find(|&i| batch.lightmaps[i] >= 0 && batch.lightmap_styles[i] < 254);
        let vertex_lit_slot = (0..4)
            .find(|&i| batch.lightmaps[i] == LIGHTMAP_BY_VERTEX && surface.vertex_styles[i] < 254);
        let color_slot = vertex_lit_slot
            .or(lightmap_slot)
            .or_else(|| (0..4).find(|&i| surface.vertex_styles[i] < 254))
            .unwrap_or(0);
        let pvs_signature = bsp
            .visibility
            .as_ref()
            .map(|vis| pvs_signature(vis, &vis.surface_clusters[batch.surface]))
            .unwrap_or_default();
        let mut triangles = Vec::new();
        for triangle in mesh.indices[batch.indices.clone()].as_chunks::<3>().0 {
            // Keep original JKA index order here. Render geometry reverses the
            // winding when converting coordinate handedness, but Raven's random
            // interval uses v1.x + v2.y + v3.z and therefore depends on order.
            let source = triangle.map(|index| mesh.vertices[index as usize]);
            triangles.push(SurfaceSpriteEffectTriangle {
                positions_jka: source.map(|vertex| vertex.position),
                normal_z: source.map(|vertex| vertex.normal[2]),
                light: source.map(|vertex| vertex.color[color_slot][2]),
            });
        }
        if triangles.is_empty() {
            continue;
        }
        for effect in &material.surface_sprite_effects {
            emitters.push(SurfaceSpriteEffectEmitter {
                texture: effect.texture,
                clamp: effect.clamp,
                blend: effect.blend,
                sprite: effect.sprite,
                triangles: triangles.clone(),
                pvs_signature: pvs_signature.clone(),
            });
        }
    }
    emitters
}

#[derive(Clone)]
pub(in crate::scene) struct GrassExternalLightmap {
    pub(in crate::scene) width: u32,
    pub(in crate::scene) height: u32,
    pub(in crate::scene) rgba: Arc<[u8]>,
}

#[derive(Clone)]
pub(in crate::scene) struct GrassLightmapSources {
    pub(in crate::scene) embedded: Arc<[u8]>,
    pub(in crate::scene) external: Arc<BTreeMap<usize, GrassExternalLightmap>>,
}

#[derive(Clone)]
pub(in crate::scene) struct GrassEmitterTriangle {
    pub(in crate::scene) positions: [[f32; 3]; 3],
    pub(in crate::scene) colors: [[u8; 4]; 3],
    pub(in crate::scene) lightmap_uv: Option<[[f32; 2]; 3]>,
    pub(in crate::scene) lightmap_page: Option<usize>,
    pub(in crate::scene) signature_id: u32,
    pub(in crate::scene) triangle_seed: u32,
    pub(in crate::scene) height: f32,
    pub(in crate::scene) density: f32,
    pub(in crate::scene) ground_tint_rgba: [u8; 4],
    pub(in crate::scene) fog_slot: u8,
}

/// Build a compact table only for local brush fogs that grass can actually inherit
/// from a BSP source surface. Global fog remains on the existing whole-scene path.
/// A u8 slot keeps the 24-byte GrassInstance stride unchanged.
pub(in crate::scene) fn grass_local_fog_slots(
    bsp: &Bsp,
    library: &BTreeMap<String, Shader>,
    global_fog_num: Option<i32>,
) -> (BTreeMap<i32, u8>, Vec<[f32; 4]>) {
    let mut slots = BTreeMap::new();
    let mut fogs = vec![[0.0; 4]]; // slot 0 = no local fog
    for fog_index in 0..bsp.fogs.len() {
        let Ok(fog_num) = i32::try_from(fog_index) else {
            break;
        };
        if global_fog_num == Some(fog_num) {
            continue;
        }
        let params = bsp_fog_params(bsp, library, fog_num);
        if params[3] <= 0.0 {
            continue;
        }
        let Ok(slot) = u8::try_from(fogs.len()) else {
            // 255 local fog slots plus slot 0 is far beyond normal JKA maps.
            break;
        };
        slots.insert(fog_num, slot);
        fogs.push(params);
    }
    (slots, fogs)
}

pub(in crate::scene) fn collect_grass_emitters(
    bsp: &Bsp,
    mesh: &jka_assets::bsp::Mesh,
    surface_materials: &[SurfaceMaterial],
    grass_ground_tints: &[[u8; 4]],
    fog_slots: &BTreeMap<i32, u8>,
) -> (Vec<GrassEmitterTriangle>, Vec<Vec<u64>>) {
    let mut emitters = Vec::new();
    let mut signature_ids = BTreeMap::<Vec<u64>, u32>::new();
    let mut signatures = Vec::<Vec<u64>>::new();
    for batch in &mesh.batches {
        let Some(material) = surface_materials.get(batch.shader) else {
            continue;
        };
        let Some(grass) = material.grass else {
            continue;
        };
        if material.hidden {
            continue;
        }
        let Some(surface) = bsp.surfaces.get(batch.surface) else {
            continue;
        };
        let lightmap_slot =
            (0..4).find(|&i| batch.lightmaps[i] >= 0 && batch.lightmap_styles[i] < 254);
        let vertex_lit_slot = (0..4)
            .find(|&i| batch.lightmaps[i] == LIGHTMAP_BY_VERTEX && surface.vertex_styles[i] < 254);
        let color_slot = vertex_lit_slot
            .or(lightmap_slot)
            .or_else(|| (0..4).find(|&i| surface.vertex_styles[i] < 254))
            .unwrap_or(0);
        let lightmap_page = lightmap_slot
            .filter(|_| !material.sky)
            .map(|slot| batch.lightmaps[slot] as usize);
        let signature = bsp
            .visibility
            .as_ref()
            .map(|vis| pvs_signature(vis, &vis.surface_clusters[batch.surface]))
            .unwrap_or_default();
        let signature_id = if let Some(&id) = signature_ids.get(&signature) {
            id
        } else {
            let id = signatures.len() as u32;
            signature_ids.insert(signature.clone(), id);
            signatures.push(signature);
            id
        };
        let ground_tint_rgba = grass_ground_tints
            .get(batch.shader)
            .copied()
            .unwrap_or([0, 0, 0, 0]);

        for (triangle_index, triangle) in mesh.indices[batch.indices.clone()]
            .as_chunks::<3>()
            .0
            .iter()
            .enumerate()
        {
            let source = [0usize, 2, 1].map(|corner| mesh.vertices[triangle[corner] as usize]);
            let positions = source.map(|vertex| render_position(vertex.position));
            let p = positions.map(Vec3::from_array);
            let cross = (p[1] - p[0]).cross(p[2] - p[0]);
            let double_area = cross.length();
            if double_area <= 1.0e-4 {
                continue;
            }
            let authored_normal = source
                .iter()
                .map(|vertex| Vec3::from_array(render_position(vertex.normal)))
                .fold(Vec3::ZERO, |sum, normal| sum + normal)
                .normalize_or_zero();
            let mut normal = cross / double_area;
            if authored_normal.length_squared() > 0.25 {
                normal = authored_normal;
            }
            if !grass.any_angle && normal.y < 0.5 {
                continue;
            }
            let colors = source.map(|vertex| vertex.color[color_slot]);
            let lightmap_uv =
                lightmap_slot.map(|slot| source.map(|vertex| vertex.lightmap_uv[slot]));
            let triangle_seed = grass_hash(
                (batch.surface as u32).wrapping_mul(0x9e37_79b9)
                    ^ (triangle_index as u32).wrapping_mul(0x85eb_ca6b)
                    ^ (batch.shader as u32).wrapping_mul(0xc2b2_ae35),
            );
            emitters.push(GrassEmitterTriangle {
                positions,
                colors,
                lightmap_uv,
                lightmap_page,
                signature_id,
                triangle_seed,
                height: grass.height.max(1.0),
                density: grass.density,
                ground_tint_rgba,
                fog_slot: fog_slots.get(&surface.fog_num).copied().unwrap_or(0),
            });
        }
    }
    (emitters, signatures)
}

/// Fingerprint of everything blade generation reads: each emitter triangle, the
/// PVS signatures its `signature_id`s resolve to, and the sampled lightmap pixels.
pub(in crate::scene) fn grass_fingerprint(
    emitters: &[GrassEmitterTriangle],
    signatures: &[Vec<u64>],
    lightmaps: &GrassLightmapSources,
) -> u64 {
    let mut fingerprint = StageFingerprint::new("grass");
    fingerprint.pod(&[emitters.len() as u64]);
    for emitter in emitters {
        fingerprint
            .pod(&emitter.positions)
            .pod(&emitter.colors)
            .pod(&[
                emitter.signature_id,
                emitter.triangle_seed,
                emitter.height.to_bits(),
                emitter.density.to_bits(),
                u32::from_le_bytes(emitter.ground_tint_rgba),
                u32::from(emitter.fog_slot),
            ])
            .debug(&emitter.lightmap_uv)
            .debug(&emitter.lightmap_page);
    }
    for signature in signatures {
        fingerprint.pod(signature);
    }
    fingerprint.bytes(&lightmaps.embedded);
    for (page, image) in lightmaps.external.iter() {
        fingerprint
            .pod(&[
                *page as u64,
                u64::from(image.width),
                u64::from(image.height),
            ])
            .bytes(&image.rgba);
    }
    fingerprint.finish()
}

pub(in crate::scene) fn grass_lightmap_sources(
    bsp: &Bsp,
    external_lightmaps: &[TextureData],
    external_lightmap_lookup: &BTreeMap<usize, usize>,
) -> GrassLightmapSources {
    let external = external_lightmap_lookup
        .iter()
        .filter_map(|(&page, &index)| {
            let image = external_lightmaps.get(index)?;
            let base_len = (image.width as usize)
                .saturating_mul(image.height as usize)
                .saturating_mul(4)
                .min(image.rgba.len());
            Some((
                page,
                GrassExternalLightmap {
                    width: image.width,
                    height: image.height,
                    rgba: Arc::from(image.rgba[..base_len].to_vec()),
                },
            ))
        })
        .collect();
    GrassLightmapSources {
        embedded: Arc::from(bsp.lightmaps.clone()),
        external: Arc::new(external),
    }
}

pub(in crate::scene) fn grass_worker_baked_lighting(
    sources: &GrassLightmapSources,
    page: usize,
    uv: [f32; 2],
) -> Option<[f32; 3]> {
    if let Some(image) = sources.external.get(&page) {
        return Some(sample_rgba_srgb_bilinear(
            &image.rgba,
            image.width,
            image.height,
            uv,
        ));
    }
    sample_embedded_lightmap_bilinear(&sources.embedded, page, uv)
}

pub(in crate::scene) fn pack_grass_height_fog_slot(height: f32, fog_slot: u8) -> f32 {
    debug_assert!(height.is_finite() && height > 0.0);
    f32::from_bits((height.to_bits() & !0xff) | u32::from(fog_slot))
}

pub(in crate::scene) fn grass_instance_height(instance: &GrassInstance) -> f32 {
    f32::from_bits(instance.height.to_bits() & !0xff)
}

pub(in crate::scene) fn generate_grass_chunk(
    emitters: Vec<GrassEmitterTriangle>,
    lightmaps: Arc<GrassLightmapSources>,
    clump_data: Arc<Vec<u8>>,
) -> (HashMap<GrassWorkerPatchKey, Vec<GrassInstance>>, usize) {
    let mut groups = HashMap::<GrassWorkerPatchKey, Vec<GrassInstance>>::new();
    let mut blade_count = 0usize;
    for emitter in emitters {
        let positions = emitter.positions.map(Vec3::from_array);
        let area = (positions[1] - positions[0])
            .cross(positions[2] - positions[0])
            .length()
            * 0.5;
        if area <= 1.0e-4 {
            continue;
        }
        let spacing = (emitter.density.abs() / GRASS_REFERENCE_SPRITE_DENSITY
            * GRASS_FULL_DENSITY_SPACING)
            .clamp(GRASS_MIN_SPACING, GRASS_MAX_SPACING);
        let desired = area / (spacing * spacing);
        let whole = desired.floor() as usize;
        let extra = if whole < GRASS_MAX_BLADES_PER_TRIANGLE
            && grass_random(emitter.triangle_seed, 0xa511_e9b3) < desired.fract()
        {
            1
        } else {
            0
        };
        let count = (whole + extra).min(GRASS_MAX_BLADES_PER_TRIANGLE);
        for blade in 0..count {
            let seed = grass_hash(emitter.triangle_seed ^ (blade as u32).wrapping_mul(0x27d4_eb2d));
            let r1 = grass_random(seed, 0x68bc_21eb).sqrt();
            let r2 = grass_random(seed, 0x02e5_be93);
            let bary = [1.0 - r1, r1 * (1.0 - r2), r1 * r2];
            let position = positions[0] * bary[0] + positions[1] * bary[1] + positions[2] * bary[2];
            let mut color = [0.0_f32; 3];
            for corner in 0..3 {
                for channel in 0..3 {
                    color[channel] +=
                        f32::from(emitter.colors[corner][channel]) / 255.0 * bary[corner];
                }
            }
            if let (Some(page), Some(uvs)) = (emitter.lightmap_page, emitter.lightmap_uv) {
                let raw_uv = [0usize, 1usize].map(|axis| {
                    uvs[0][axis] * bary[0] + uvs[1][axis] * bary[1] + uvs[2][axis] * bary[2]
                });
                if let Some(baked) = grass_worker_baked_lighting(&lightmaps, page, raw_uv) {
                    color = baked;
                }
            }
            let cell_x = (position.x / GRASS_PATCH_SIZE).floor() as i32;
            let cell_z = (position.z / GRASS_PATCH_SIZE).floor() as i32;
            groups
                .entry(GrassWorkerPatchKey {
                    cell_x,
                    cell_z,
                    signature_id: emitter.signature_id,
                })
                .or_default()
                .push({
                    let packed_clump = crate::grass::pack_static_clump_alpha(
                        position.to_array(),
                        &clump_data,
                        emitter.ground_tint_rgba[3] >= 128,
                    );
                    let baked_light_rgba = [
                        (color[0].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                        (color[1].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                        (color[2].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
                        packed_clump as u8,
                    ];
                    let mut ground_tint_rgba = emitter.ground_tint_rgba;
                    ground_tint_rgba[3] = (packed_clump >> 8) as u8;
                    GrassInstance {
                        position: position.to_array(),
                        height: pack_grass_height_fog_slot(emitter.height, emitter.fog_slot),
                        baked_light_rgba,
                        ground_tint_rgba,
                    }
                });
            blade_count += 1;
        }
    }
    (groups, blade_count)
}

pub(in crate::scene) fn grass_instance_lod_key(instance: &GrassInstance) -> u32 {
    let mut value = instance.position[0].to_bits().wrapping_mul(0x9e37_79b9)
        ^ instance.position[1].to_bits().rotate_left(11)
        ^ instance.position[2].to_bits().wrapping_mul(0x85eb_ca6b);
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^ (value >> 16)
}

pub(in crate::scene) fn finish_grass_patch(
    key: GrassPatchKey,
    mut instances: Vec<GrassInstance>,
) -> Option<GrassPatch> {
    if instances.is_empty() {
        return None;
    }
    // Sorting once on the map workers means renderer upload can copy the compact
    // 24-byte records directly instead of re-sorting ~29M blades on the render thread.
    instances.sort_unstable_by_key(grass_instance_lod_key);
    let mut minimum = Vec3::splat(f32::INFINITY);
    let mut maximum = Vec3::splat(f32::NEG_INFINITY);
    let mut max_height = 0.0_f32;
    for instance in &instances {
        let position = Vec3::from_array(instance.position);
        minimum = minimum.min(position);
        maximum = maximum.max(position);
        max_height = max_height.max(grass_instance_height(instance));
    }
    maximum.y += max_height * 4.0;
    minimum.y -= max_height * 0.5;
    let center = (minimum + maximum) * 0.5;
    let radius = (maximum - center).length();
    Some(GrassPatch {
        instances,
        pvs_signature: key.pvs_signature,
        center: center.to_array(),
        radius,
    })
}

pub(in crate::scene) fn finish_grass_patches(
    groups: BTreeMap<GrassPatchKey, Vec<GrassInstance>>,
) -> Vec<GrassPatch> {
    groups
        .into_iter()
        .filter_map(|(key, instances)| finish_grass_patch(key, instances))
        .collect()
}

pub(in crate::scene) fn finish_grass_patches_with_jobs(
    groups: BTreeMap<GrassPatchKey, Vec<GrassInstance>>,
    jobs: Option<&MapJobPool>,
) -> Result<(Vec<GrassPatch>, f64), String> {
    let Some(jobs) = jobs.filter(|_| groups.len() > 64) else {
        let started = Instant::now();
        let patches = finish_grass_patches(groups);
        return Ok((patches, started.elapsed().as_secs_f64() * 1000.0));
    };
    let entries = groups.into_iter().collect::<Vec<_>>();
    let worker_count = jobs.worker_count().min(entries.len()).max(1);
    let chunk_size = entries.len().div_ceil(worker_count);
    let mut handles = Vec::new();
    let mut base = 0usize;
    let mut iter = entries.into_iter();
    loop {
        let chunk = iter.by_ref().take(chunk_size).collect::<Vec<_>>();
        if chunk.is_empty() {
            break;
        }
        let chunk_base = base;
        base += chunk.len();
        handles.push(jobs.submit(Task::MapGrass, move || {
            let started = Instant::now();
            let patches = chunk
                .into_iter()
                .enumerate()
                .filter_map(|(offset, (key, instances))| {
                    finish_grass_patch(key, instances).map(|patch| (chunk_base + offset, patch))
                })
                .collect::<Vec<_>>();
            (patches, started.elapsed().as_secs_f64() * 1000.0)
        })?);
    }
    let mut ordered = Vec::<(usize, GrassPatch)>::new();
    let mut cpu_ms = 0.0_f64;
    for handle in handles {
        let (mut patches, worker_ms) = handle.join()?;
        cpu_ms += worker_ms;
        ordered.append(&mut patches);
    }
    ordered.sort_unstable_by_key(|(index, _)| *index);
    Ok((
        ordered.into_iter().map(|(_, patch)| patch).collect(),
        cpu_ms,
    ))
}

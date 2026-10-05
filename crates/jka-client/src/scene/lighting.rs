//! Lighting.
use crate::scene::{
    materials, render_position, AssetSearchPath, BTreeSet, Bsp, ClassicEntityLightGrid,
    ClassicLightGridCell, CullMode, DirectionalSun, DrawBatch, DrawClass, DynamicLight,
    DynamicLightFalloff, GpuVertex, StaticLightGrid, SurfaceMaterial, Vec3, VoxelProbeGi,
    MAX_EXTRACTED_SURFACE_LIGHTS, MAX_SURFACE_LIGHT_SUBDIVISION_DEPTH, VOXEL_GI_MAX_AXIS,
    VOXEL_GI_MAX_RADIANCE, VOXEL_GI_MIN_CELL_SIZE, VOXEL_GI_PROPAGATION,
    VOXEL_GI_PROPAGATION_STEPS,
};
use crate::scene::{parse_light_triplet, parse_map_f32, SourceMapLighting};

pub(in crate::scene) fn decode_lightgrid_direction(lat_long: [u8; 2]) -> [f32; 3] {
    // OpenJK/Q3 stores longitude first and latitude second. The original
    // renderer indexes its 1024-entry trig table with byte * 4, which is
    // exactly byte * 2*pi/256 in radians.
    let longitude = f32::from(lat_long[0]) * std::f32::consts::TAU / 256.0;
    let latitude = f32::from(lat_long[1]) * std::f32::consts::TAU / 256.0;
    let sin_longitude = longitude.sin();
    let jka = [
        latitude.cos() * sin_longitude,
        latitude.sin() * sin_longitude,
        longitude.cos(),
    ];
    // JKA -> renderer coordinates: [x, z, -y].
    let render = [jka[0], jka[2], -jka[1]];
    let length = (render[0] * render[0] + render[1] * render[1] + render[2] * render[2])
        .sqrt()
        .max(1e-6);
    [render[0] / length, render[1] / length, render[2] / length]
}

pub(in crate::scene) fn light_luminance(rgb: [f32; 3]) -> f32 {
    rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722
}

pub(in crate::scene) fn smoothstep_range(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub(in crate::scene) fn encode_unit_channel(value: f32) -> u8 {
    ((value.clamp(0.0, 1.0) * 255.0) + 0.5) as u8
}

pub(in crate::scene) fn prepare_static_light_grid(
    bsp: &Bsp,
    assets: &mut AssetSearchPath,
    map_name: &str,
    warnings: &mut Vec<String>,
) -> Result<Option<StaticLightGrid>, String> {
    let Some(grid) = bsp.light_grid.as_ref() else {
        return Ok(None);
    };
    let cell_count = grid.cell_count();
    if cell_count == 0 {
        return Ok(None);
    }

    // Rend2/q3map2 may ship an expanded HDR lightgrid alongside the BSP.
    // It contains six little-endian f32 values per grid point: ambient RGB,
    // directed RGB. We only need scale-independent directionality/chromaticity
    // here because the BSP lightmap still provides the baked irradiance scale.
    let raw_path = format!("maps/{map_name}/lightgrid.raw");
    let external = assets
        .read(&raw_path, cell_count.saturating_mul(24).saturating_add(1))
        .map_err(|error| error.to_string())?;
    let external_values = if let Some(asset) = external {
        if asset.bytes.len() == cell_count * 24 {
            let values = asset
                .bytes
                .chunks_exact(4)
                .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
                .collect::<Vec<_>>();
            warnings.push(format!(
                "{map_name}: using Rend2 HDR lightgrid {} ({} grid points)",
                raw_path, cell_count
            ));
            Some(values)
        } else {
            warnings.push(format!(
                "{map_name}: ignored {} because it is {} bytes; expected {}",
                raw_path,
                asset.bytes.len(),
                cell_count * 24
            ));
            None
        }
    } else {
        None
    };

    let mut direction_rgba = Vec::with_capacity(cell_count * 4);
    let mut lighting_rgba = Vec::with_capacity(cell_count * 4);
    let mut classic_cells = Vec::with_capacity(cell_count);
    // Keep the six ambient-cube irradiances in float until every probe has been
    // seen, then choose one map-wide scale so HDR lightgrid.raw values are not
    // clipped by the RGBA8 volume texture. Face order: +X,-X,+Y,-Y,+Z,-Z.
    let mut ambient_cube_faces = vec![[[0.0_f32; 3]; 6]; cell_count];
    let mut ambient_cube_peak = 255.0_f32;
    for cell in 0..cell_count {
        let sample = grid.sample_for_cell(cell);
        let style = sample.and_then(|sample| sample.styles.iter().position(|style| *style < 254));
        let valid = sample.is_some() && style.is_some();
        let direction = sample
            .map(|sample| decode_lightgrid_direction(sample.lat_long))
            .unwrap_or([0.0, 1.0, 0.0]);

        let (ambient, directed) = if let Some(values) = external_values.as_ref() {
            let base = cell * 6;
            let clean = |value: f32| {
                if value.is_finite() {
                    value.max(0.0)
                } else {
                    0.0
                }
            };
            (
                [
                    clean(values[base]),
                    clean(values[base + 1]),
                    clean(values[base + 2]),
                ],
                [
                    clean(values[base + 3]),
                    clean(values[base + 4]),
                    clean(values[base + 5]),
                ],
            )
        } else if let (Some(sample), Some(style)) = (sample, style) {
            (
                sample.ambient_light[style].map(f32::from),
                sample.direct_light[style].map(f32::from),
            )
        } else {
            ([0.0; 3], [0.0; 3])
        };

        // Keep a second absolute-light representation for dynamic entities.
        // OpenJK's entity path consumes every active style slot (styleColors
        // initialize to white), whereas the scale-free PBR representation above
        // only needs one representative chroma/direction sample.
        let classic_valid = sample.is_some_and(|sample| sample.styles[0] != 255);
        let (classic_ambient, classic_directed) = if classic_valid {
            if let Some(values) = external_values.as_ref() {
                // Rend2 loads lightgrid.raw as radiance / PI, then converts back
                // to the renderer's 0..255 light units during entity sampling.
                let base = cell * 6;
                let convert = |value: f32| {
                    if value.is_finite() {
                        value.max(0.0) * (255.0 / std::f32::consts::PI)
                    } else {
                        0.0
                    }
                };
                (
                    [
                        convert(values[base]),
                        convert(values[base + 1]),
                        convert(values[base + 2]),
                    ],
                    [
                        convert(values[base + 3]),
                        convert(values[base + 4]),
                        convert(values[base + 5]),
                    ],
                )
            } else if let Some(sample) = sample {
                let mut classic_ambient = [0.0_f32; 3];
                let mut classic_directed = [0.0_f32; 3];
                for style_index in 0..sample.styles.len() {
                    if sample.styles[style_index] == 255 {
                        break;
                    }
                    for channel in 0..3 {
                        classic_ambient[channel] +=
                            f32::from(sample.ambient_light[style_index][channel]);
                        classic_directed[channel] +=
                            f32::from(sample.direct_light[style_index][channel]);
                    }
                }
                (classic_ambient, classic_directed)
            } else {
                ([0.0; 3], [0.0; 3])
            }
        } else {
            ([0.0; 3], [0.0; 3])
        };
        classic_cells.push(ClassicLightGridCell {
            ambient: classic_ambient,
            directed: classic_directed,
            direction,
            sun_weight: 0.0,
            valid: classic_valid,
        });

        let axes = [
            [1.0_f32, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
        ];
        for (face_index, axis) in axes.into_iter().enumerate() {
            let ndotl =
                (axis[0] * direction[0] + axis[1] * direction[1] + axis[2] * direction[2]).max(0.0);
            let irradiance = [
                ambient[0] + directed[0] * ndotl,
                ambient[1] + directed[1] * ndotl,
                ambient[2] + directed[2] * ndotl,
            ];
            ambient_cube_peak =
                ambient_cube_peak.max(irradiance.into_iter().fold(0.0_f32, f32::max));
            ambient_cube_faces[cell][face_index] = irradiance;
        }

        let ambient_luma = light_luminance(ambient);
        let directed_luma = light_luminance(directed);
        let directional_fraction = if valid && ambient_luma + directed_luma > 1e-6 {
            directed_luma / (ambient_luma + directed_luma)
        } else {
            0.0
        };
        let direct_max = directed.into_iter().fold(0.0_f32, f32::max);
        let chroma = if direct_max > 1e-6 {
            directed.map(|value| value / direct_max)
        } else {
            [1.0; 3]
        };

        direction_rgba.extend_from_slice(&[
            encode_unit_channel(direction[0] * 0.5 + 0.5),
            encode_unit_channel(direction[1] * 0.5 + 0.5),
            encode_unit_channel(direction[2] * 0.5 + 0.5),
            encode_unit_channel(directional_fraction),
        ]);
        lighting_rgba.extend_from_slice(&[
            encode_unit_channel(chroma[0]),
            encode_unit_channel(chroma[1]),
            encode_unit_channel(chroma[2]),
            if valid { 255 } else { 0 },
        ]);
    }

    let bx = grid.bounds[0] as usize;
    let by = grid.bounds[1] as usize;
    let bz = grid.bounds[2] as usize;
    let atlas_height = by * 2;
    let atlas_depth = bz * 3;
    let mut irradiance_volume_rgba = vec![0_u8; bx * atlas_height * atlas_depth * 4];
    let encode_irradiance = |value: f32| -> u8 {
        ((value.max(0.0) / ambient_cube_peak).clamp(0.0, 1.0) * 255.0 + 0.5) as u8
    };
    for z in 0..bz {
        for y in 0..by {
            for x in 0..bx {
                let cell = x + bx * (y + by * z);
                for axis in 0..3 {
                    for negative in 0..2 {
                        let face = axis * 2 + negative;
                        let atlas_y = y + negative * by;
                        let atlas_z = z + axis * bz;
                        let dst = 4 * (x + bx * (atlas_y + atlas_height * atlas_z));
                        let rgb = ambient_cube_faces[cell][face];
                        irradiance_volume_rgba[dst] = encode_irradiance(rgb[0]);
                        irradiance_volume_rgba[dst + 1] = encode_irradiance(rgb[1]);
                        irradiance_volume_rgba[dst + 2] = encode_irradiance(rgb[2]);
                        irradiance_volume_rgba[dst + 3] = 255;
                    }
                }
            }
        }
    }
    let irradiance_intensity = ambient_cube_peak / 255.0;

    warnings.push(format!(
        "{map_name}: Bevy irradiance volume {}x{}x{} ambient-cube atlas generated from the BSP lightgrid",
        bx, atlas_height, atlas_depth
    ));
    warnings.push(format!(
        "{map_name}: static lightgrid {}x{}x{} available for PBR directional baked lighting and classic entity lighting{}",
        grid.bounds[0],
        grid.bounds[1],
        grid.bounds[2],
        if external_values.is_some() {
            " (HDR companion)"
        } else {
            ""
        }
    ));

    Ok(Some(StaticLightGrid {
        // Preserve JKA X/Y/Z indexing in the 3D texture; the shader converts
        // render-space positions back to JKA coordinates before sampling.
        origin: grid.origin,
        size: grid.size,
        bounds: [
            grid.bounds[0] as u32,
            grid.bounds[1] as u32,
            grid.bounds[2] as u32,
        ],
        direction_rgba,
        lighting_rgba,
        irradiance_volume_rgba,
        irradiance_intensity,
        external_hdr: external_values.is_some(),
        classic_entity_grid: ClassicEntityLightGrid {
            origin: grid.origin,
            size: grid.size,
            bounds: [
                grid.bounds[0] as u32,
                grid.bounds[1] as u32,
                grid.bounds[2] as u32,
            ],
            external_hdr: external_values.is_some(),
            cells: classic_cells,
            map_toward_sun: None,
        },
    }))
}

#[derive(Debug, Clone, Copy)]
pub(in crate::scene) struct SurfaceLightCandidate {
    pub(in crate::scene) light: DynamicLight,
    pub(in crate::scene) weight: f32,
}

pub(in crate::scene) fn surface_light_luminance(color: [f32; 3]) -> f32 {
    color[0] * 0.2126 + color[1] * 0.7152 + color[2] * 0.0722
}

pub(in crate::scene) fn append_surface_light_triangle(
    candidates: &mut Vec<SurfaceLightCandidate>,
    positions: [Vec3; 3],
    normal: Vec3,
    authored: materials::SurfaceLight,
    two_sided: bool,
) {
    if !authored.value.is_finite() || authored.value <= 0.0 {
        return;
    }

    // q3map2 subdivides emitting draw surfaces before creating its area lights.
    // Do the same geometrically so a large emissive face is represented by
    // several local sources rather than one giant point light. Longest-edge
    // bisection is deterministic and also works for already-tessellated patches.
    let max_edge = authored.subdivide.max(16.0);
    let max_edge_sq = max_edge * max_edge;
    let mut pending = vec![(positions, 0_u8)];
    while let Some((triangle, depth)) = pending.pop() {
        let edge_sq = [
            triangle[0].distance_squared(triangle[1]),
            triangle[1].distance_squared(triangle[2]),
            triangle[2].distance_squared(triangle[0]),
        ];
        let (longest, longest_sq) = edge_sq
            .into_iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.total_cmp(b))
            .unwrap_or((0, 0.0));
        if longest_sq > max_edge_sq && depth < MAX_SURFACE_LIGHT_SUBDIVISION_DEPTH {
            let (a, b, c) = match longest {
                0 => (triangle[0], triangle[1], triangle[2]),
                1 => (triangle[1], triangle[2], triangle[0]),
                _ => (triangle[2], triangle[0], triangle[1]),
            };
            let midpoint = (a + b) * 0.5;
            pending.push(([a, midpoint, c], depth + 1));
            pending.push(([midpoint, b, c], depth + 1));
            continue;
        }

        let area = 0.5
            * (triangle[1] - triangle[0])
                .cross(triangle[2] - triangle[0])
                .length();
        if !area.is_finite() || area <= 0.01 {
            continue;
        }
        let emitter_normal = normal.normalize_or_zero();
        if emitter_normal.length_squared() <= 1e-6 {
            continue;
        }

        let centroid = (triangle[0] + triangle[1] + triangle[2]) / 3.0;
        // Normalize each generated sample by the authored subdivision cell area.
        // This keeps total emitted energy approximately proportional to surface
        // area and avoids depending on the BSP's incidental triangle density.
        let area_scale = (area / (max_edge * max_edge)).clamp(0.0, 1.0);
        let intensity = ((authored.value / 300.0) * area_scale).clamp(0.0, 8.0);
        if intensity <= 1e-5 {
            continue;
        }
        let radius = authored.value.max(max_edge * 2.0).clamp(64.0, 4096.0);
        let luminance = surface_light_luminance(authored.color).max(0.0);
        candidates.push(SurfaceLightCandidate {
            light: DynamicLight {
                // Keep the sample just in front of its source plane so receiving
                // surfaces that meet the emitter do not start numerically behind it.
                position: (centroid + emitter_normal * 8.0).to_array(),
                color: authored.color,
                radius,
                intensity,
                falloff: DynamicLightFalloff::Smooth,
                surface_lighting: true,
                emitter_normal: emitter_normal.to_array(),
                emitter_two_sided: two_sided,
                angle_attenuation: true,
                angle_scale: 0.0,
                extra_distance: 0.0,
            },
            // Modern mesh-light samplers prioritize emitting triangles by area
            // times luminance. Include q3map's authored strength in that weight.
            weight: area * luminance * authored.value,
        });
    }
}

pub(in crate::scene) fn bsp_surface_lights(
    mesh: &jka_assets::bsp::Mesh,
    materials: &[SurfaceMaterial],
) -> (Vec<DynamicLight>, usize) {
    let mut candidates = Vec::new();
    for batch in &mesh.batches {
        let Some(material) = materials.get(batch.shader) else {
            continue;
        };
        let Some(authored) = material.surface_light else {
            continue;
        };
        let two_sided = material.cull == CullMode::None;
        for triangle in mesh.indices[batch.indices.clone()].as_chunks::<3>().0 {
            let vertices = (*triangle).map(|index| &mesh.vertices[index as usize]);
            let positions =
                vertices.map(|vertex| Vec3::from_array(render_position(vertex.position)));
            let normal = vertices
                .map(|vertex| Vec3::from_array(render_position(vertex.normal)))
                .into_iter()
                .sum::<Vec3>()
                .normalize_or_zero();
            append_surface_light_triangle(&mut candidates, positions, normal, authored, two_sided);
        }
    }

    let candidate_count = candidates.len();
    candidates.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    candidates.truncate(MAX_EXTRACTED_SURFACE_LIGHTS);
    (
        candidates
            .into_iter()
            .map(|candidate| candidate.light)
            .collect(),
        candidate_count,
    )
}

pub(in crate::scene) fn voxel_gi_index(x: u32, y: u32, z: u32, bounds: [u32; 3]) -> usize {
    (z as usize * bounds[1] as usize + y as usize) * bounds[0] as usize + x as usize
}

pub(in crate::scene) fn voxel_gi_coords_for_position(
    position: Vec3,
    minimum: Vec3,
    cell_size: f32,
    bounds: [u32; 3],
) -> Option<[u32; 3]> {
    let relative = (position - minimum) / cell_size;
    if relative.min_element() < 0.0 {
        return None;
    }
    let coords = [
        relative.x.floor() as u32,
        relative.y.floor() as u32,
        relative.z.floor() as u32,
    ];
    (coords[0] < bounds[0] && coords[1] < bounds[1] && coords[2] < bounds[2]).then_some(coords)
}

pub(in crate::scene) fn voxel_gi_nearest_free(
    position: Vec3,
    minimum: Vec3,
    cell_size: f32,
    bounds: [u32; 3],
    occupied: &[bool],
) -> Option<usize> {
    let base = voxel_gi_coords_for_position(position, minimum, cell_size, bounds)?;
    let mut best = None::<(usize, f32)>;
    for radius in 0_i32..=3 {
        for dz in -radius..=radius {
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    if dx.abs().max(dy.abs()).max(dz.abs()) != radius {
                        continue;
                    }
                    let x = base[0] as i32 + dx;
                    let y = base[1] as i32 + dy;
                    let z = base[2] as i32 + dz;
                    if x < 0
                        || y < 0
                        || z < 0
                        || x >= bounds[0] as i32
                        || y >= bounds[1] as i32
                        || z >= bounds[2] as i32
                    {
                        continue;
                    }
                    let index = voxel_gi_index(x as u32, y as u32, z as u32, bounds);
                    if occupied[index] {
                        continue;
                    }
                    let center = minimum
                        + Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5) * cell_size;
                    let distance = center.distance_squared(position);
                    if best.is_none_or(|(_, best_distance)| distance < best_distance) {
                        best = Some((index, distance));
                    }
                }
            }
        }
        if best.is_some() {
            break;
        }
    }
    best.map(|(index, _)| index)
}

pub(in crate::scene) fn voxel_gi_add_source(target: &mut [Vec3], index: usize, energy: Vec3) {
    if let Some(value) = target.get_mut(index) {
        *value = (*value + energy).min(Vec3::splat(VOXEL_GI_MAX_RADIANCE));
    }
}

pub(in crate::scene) fn voxel_gi_add_source_max(target: &mut [Vec3], index: usize, energy: Vec3) {
    if let Some(value) = target.get_mut(index) {
        *value = (*value).max(energy).min(Vec3::splat(VOXEL_GI_MAX_RADIANCE));
    }
}

pub(in crate::scene) fn voxel_gi_ray_reaches_outside(
    start: Vec3,
    direction: Vec3,
    minimum: Vec3,
    cell_size: f32,
    bounds: [u32; 3],
    occupied: &[bool],
) -> bool {
    let direction = direction.normalize_or_zero();
    if direction.length_squared() <= 1e-6 {
        return false;
    }
    let mut position = start;
    let step = cell_size * 0.55;
    let max_steps = ((bounds[0] + bounds[1] + bounds[2]) * 4).max(16);
    let mut previous = None;
    for _ in 0..max_steps {
        position += direction * step;
        let Some([x, y, z]) = voxel_gi_coords_for_position(position, minimum, cell_size, bounds)
        else {
            return true;
        };
        let index = voxel_gi_index(x, y, z, bounds);
        if previous == Some(index) {
            continue;
        }
        previous = Some(index);
        if occupied[index] {
            return false;
        }
    }
    false
}

pub(in crate::scene) fn voxel_gi_propagate(
    source: &[Vec3],
    occupied: &[bool],
    bounds: [u32; 3],
) -> Vec<Vec3> {
    let mut current = source.to_vec();
    let mut next = vec![Vec3::ZERO; source.len()];
    for _ in 0..VOXEL_GI_PROPAGATION_STEPS {
        next.copy_from_slice(source);
        for z in 0..bounds[2] {
            for y in 0..bounds[1] {
                for x in 0..bounds[0] {
                    let index = voxel_gi_index(x, y, z, bounds);
                    if occupied[index] {
                        next[index] = Vec3::ZERO;
                        continue;
                    }
                    let mut neighbours = Vec3::ZERO;
                    if x > 0 {
                        neighbours += current[voxel_gi_index(x - 1, y, z, bounds)];
                    }
                    if x + 1 < bounds[0] {
                        neighbours += current[voxel_gi_index(x + 1, y, z, bounds)];
                    }
                    if y > 0 {
                        neighbours += current[voxel_gi_index(x, y - 1, z, bounds)];
                    }
                    if y + 1 < bounds[1] {
                        neighbours += current[voxel_gi_index(x, y + 1, z, bounds)];
                    }
                    if z > 0 {
                        neighbours += current[voxel_gi_index(x, y, z - 1, bounds)];
                    }
                    if z + 1 < bounds[2] {
                        neighbours += current[voxel_gi_index(x, y, z + 1, bounds)];
                    }
                    next[index] = (source[index] + neighbours * (VOXEL_GI_PROPAGATION / 6.0))
                        .min(Vec3::splat(VOXEL_GI_MAX_RADIANCE));
                }
            }
        }
        std::mem::swap(&mut current, &mut next);
    }
    current
        .into_iter()
        .zip(source)
        .zip(occupied)
        .map(|((field, source), occupied)| {
            if *occupied {
                Vec3::ZERO
            } else {
                (field - *source * 0.75).max(Vec3::ZERO)
            }
        })
        .collect()
}

pub(in crate::scene) fn build_voxel_probe_gi(
    vertices: &[GpuVertex],
    batches: &[DrawBatch],
    lights: &[DynamicLight],
    sun: Option<DirectionalSun>,
) -> Option<VoxelProbeGi> {
    if vertices.is_empty() || (lights.is_empty() && sun.is_none()) {
        return None;
    }

    let mut ranges = BTreeSet::<(u32, u32)>::new();
    for batch in batches {
        if !matches!(batch.pipeline.class, DrawClass::Opaque | DrawClass::Mask) {
            continue;
        }
        ranges.insert((batch.vertices.start, batch.vertices.end));
    }
    if ranges.is_empty() {
        return None;
    }

    let mut world_min = Vec3::splat(f32::INFINITY);
    let mut world_max = Vec3::splat(f32::NEG_INFINITY);
    for &(start, end) in &ranges {
        for vertex in &vertices[start as usize..(end as usize).min(vertices.len())] {
            let position = Vec3::from_array(vertex.position);
            if position.is_finite() {
                world_min = world_min.min(position);
                world_max = world_max.max(position);
            }
        }
    }
    if !world_min.is_finite() || !world_max.is_finite() {
        return None;
    }

    let extent = (world_max - world_min).max(Vec3::splat(1.0));
    let longest = extent.max_element();
    let cell_size = (longest / (VOXEL_GI_MAX_AXIS as f32 - 4.0)).max(VOXEL_GI_MIN_CELL_SIZE);
    let minimum = world_min - Vec3::splat(cell_size * 1.5);
    let maximum = world_max + Vec3::splat(cell_size * 1.5);
    let padded_extent = maximum - minimum;
    let bounds = [
        ((padded_extent.x / cell_size).ceil() as u32).clamp(4, VOXEL_GI_MAX_AXIS),
        ((padded_extent.y / cell_size).ceil() as u32).clamp(4, VOXEL_GI_MAX_AXIS),
        ((padded_extent.z / cell_size).ceil() as u32).clamp(4, VOXEL_GI_MAX_AXIS),
    ];
    let cell_count = bounds[0] as usize * bounds[1] as usize * bounds[2] as usize;
    let mut occupied = vec![false; cell_count];
    let mut surface_normals = vec![Vec3::ZERO; cell_count];

    // Conservative surface voxelization. Sampling at <= half-cell spacing keeps
    // thin BSP walls continuous without the large overfill caused by rasterizing
    // each triangle's full voxel AABB.
    for &(start, end) in &ranges {
        let slice = &vertices[start as usize..(end as usize).min(vertices.len())];
        for triangle in slice.chunks_exact(3) {
            let p0 = Vec3::from_array(triangle[0].position);
            let p1 = Vec3::from_array(triangle[1].position);
            let p2 = Vec3::from_array(triangle[2].position);
            if !(p0.is_finite() && p1.is_finite() && p2.is_finite()) {
                continue;
            }
            let triangle_normal = triangle
                .iter()
                .map(|vertex| Vec3::from_array(vertex.normal))
                .sum::<Vec3>()
                .normalize_or_zero();
            let longest_edge = p0.distance(p1).max(p1.distance(p2)).max(p2.distance(p0));
            let steps = ((longest_edge / (cell_size * 0.48)).ceil() as usize).clamp(1, 128);
            for i in 0..=steps {
                for j in 0..=(steps - i) {
                    let a = i as f32 / steps as f32;
                    let b = j as f32 / steps as f32;
                    let c = 1.0 - a - b;
                    let point = p0 * c + p1 * a + p2 * b;
                    if let Some([x, y, z]) =
                        voxel_gi_coords_for_position(point, minimum, cell_size, bounds)
                    {
                        let index = voxel_gi_index(x, y, z, bounds);
                        occupied[index] = true;
                        surface_normals[index] += triangle_normal;
                    }
                }
            }
            let centroid = (p0 + p1 + p2) / 3.0;
            if let Some([x, y, z]) =
                voxel_gi_coords_for_position(centroid, minimum, cell_size, bounds)
            {
                let index = voxel_gi_index(x, y, z, bounds);
                occupied[index] = true;
                surface_normals[index] += triangle_normal;
            }
        }
    }

    let occupied_voxels = occupied.iter().filter(|occupied| **occupied).count();
    let mut point_source = vec![Vec3::ZERO; cell_count];
    let mut area_source = vec![Vec3::ZERO; cell_count];
    let mut deposited = 0_usize;

    // Treat direct sun on exposed world surfaces as the first bounce source.
    // The coarse voxel ray is intentionally map-load work; runtime shading only
    // pays for the resulting filtered probe sample.
    if let Some(sun) = sun.filter(|sun| sun.intensity.is_finite() && sun.intensity > 0.0) {
        let to_sun = -Vec3::from_array(sun.direction).normalize_or_zero();
        let sun_color = Vec3::from_array(sun.color).max(Vec3::ZERO);
        let sun_energy = sun_color * (sun.intensity / 300.0).clamp(0.05, 4.0) * 0.55;
        for z in 0..bounds[2] {
            for y in 0..bounds[1] {
                for x in 0..bounds[0] {
                    let surface_index = voxel_gi_index(x, y, z, bounds);
                    if !occupied[surface_index] {
                        continue;
                    }
                    let normal = surface_normals[surface_index].normalize_or_zero();
                    let facing = normal.dot(to_sun).max(0.0);
                    if facing <= 0.05 {
                        continue;
                    }
                    let surface_center = minimum
                        + Vec3::new(x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5) * cell_size;
                    let source_position = surface_center + normal * cell_size * 0.72;
                    let Some(source_index) = voxel_gi_nearest_free(
                        source_position,
                        minimum,
                        cell_size,
                        bounds,
                        &occupied,
                    ) else {
                        continue;
                    };
                    let source_center = {
                        let sx = source_index % bounds[0] as usize;
                        let sy = (source_index / bounds[0] as usize) % bounds[1] as usize;
                        let sz = source_index / (bounds[0] as usize * bounds[1] as usize);
                        minimum
                            + Vec3::new(sx as f32 + 0.5, sy as f32 + 0.5, sz as f32 + 0.5)
                                * cell_size
                    };
                    if !voxel_gi_ray_reaches_outside(
                        source_center,
                        to_sun,
                        minimum,
                        cell_size,
                        bounds,
                        &occupied,
                    ) {
                        continue;
                    }
                    voxel_gi_add_source_max(&mut point_source, source_index, sun_energy * facing);
                    deposited += 1;
                }
            }
        }
    }

    for light in lights {
        if !light.intensity.is_finite() || light.intensity <= 0.0 {
            continue;
        }
        let normal = Vec3::from_array(light.emitter_normal).normalize_or_zero();
        let is_area = normal.length_squared() > 1e-6;
        let radius_scale = (light.radius.max(64.0) / 512.0).sqrt().clamp(0.45, 1.75);
        // Source-map q3map lights store compiler photons/255 in `intensity` so
        // direct rendering can match baked luxels. Probe-GI authoring historically
        // expects the engine's ~light/300 scale; convert only those compiler lights
        // back so enabling source-map preview cannot saturate the GI volume.
        let gi_intensity = if light.falloff == DynamicLightFalloff::Smooth {
            light.intensity
        } else {
            light.intensity * Q3MAP_LIGHTMAP_BYTE_SCALE / (Q3MAP_POINT_SCALE * 300.0)
        };
        let energy = Vec3::from_array(light.color).max(Vec3::ZERO)
            * gi_intensity
            * radius_scale
            * if is_area { 0.65 } else { 0.5 };
        if energy.max_element() <= 1e-5 {
            continue;
        }

        let position = Vec3::from_array(light.position);
        let mut positions = Vec::with_capacity(2);
        if is_area {
            positions.push(position + normal * cell_size * 0.65);
            if light.emitter_two_sided {
                positions.push(position - normal * cell_size * 0.65);
            }
        } else {
            positions.push(position);
        }
        for source_position in positions {
            let Some(index) =
                voxel_gi_nearest_free(source_position, minimum, cell_size, bounds, &occupied)
            else {
                continue;
            };
            if is_area {
                voxel_gi_add_source(&mut area_source, index, energy);
            } else {
                voxel_gi_add_source(&mut point_source, index, energy);
            }
            deposited += 1;
        }
    }
    if deposited == 0 {
        return None;
    }

    let point_field = voxel_gi_propagate(&point_source, &occupied, bounds);
    let area_field = voxel_gi_propagate(&area_source, &occupied, bounds);
    let to_rgba = |field: Vec<Vec3>| {
        field
            .into_iter()
            .zip(&occupied)
            .map(|(value, occupied)| {
                let encode = |channel: f32| {
                    ((channel.clamp(0.0, VOXEL_GI_MAX_RADIANCE) / VOXEL_GI_MAX_RADIANCE * 255.0)
                        + 0.5) as u8
                };
                [
                    encode(value.x),
                    encode(value.y),
                    encode(value.z),
                    if *occupied { 0 } else { 255 },
                ]
            })
            .collect::<Vec<_>>()
    };

    Some(VoxelProbeGi {
        origin: (minimum + Vec3::splat(cell_size * 0.5)).to_array(),
        cell_size,
        bounds,
        point_rgba: to_rgba(point_field),
        area_rgba: to_rgba(area_field),
        occupied_voxels,
    })
}

pub(in crate::scene) const STATIC_BSP_AO_CACHE_VERSION: u32 = 11;

// NetRadiant Custom q3map2 defaults for the Quake3/JKA lighting model. Keep
// these next to source-map parsing: runtime/FX dlights intentionally retain the
// engine's existing response and do not use compiler-space photon units.
pub(in crate::scene) const Q3MAP_POINT_SCALE: f32 = 7500.0;

pub(in crate::scene) const Q3MAP_LINEAR_SCALE: f32 = 1.0 / 8000.0;

pub(in crate::scene) const Q3MAP_FALLOFF_TOLERANCE: f32 = 1.0;

pub(in crate::scene) const Q3MAP_LIGHTMAP_BYTE_SCALE: f32 = 255.0;

pub(in crate::scene) fn q3map_color_normalize(color: Vec3) -> Vec3 {
    let max_component = color.max_element();
    if max_component > 0.0 {
        color / max_component
    } else {
        Vec3::ONE
    }
}

pub(in crate::scene) fn map_world_lighting(
    world: &jka_assets::map::MapEntity,
) -> SourceMapLighting {
    // NetRadiant Custom LightWorld(): worldspawn `_color` tints both `_ambient`
    // and `_minlight`. Missing/zero color becomes white. The active JA profile
    // reports `_color colorspace: linear`, so source-map preview keeps authored
    // components in that space instead of inventing an sRGB transform here.
    let color = world
        .properties
        .get("_color")
        .and_then(|value| parse_light_triplet(value))
        .map(Vec3::from_array)
        .filter(|value| value.length_squared() > 0.0)
        .unwrap_or(Vec3::ONE);
    let ambient = parse_map_f32(world, "_ambient")
        .or_else(|| parse_map_f32(world, "ambient"))
        .unwrap_or(0.0);
    let minlight = parse_map_f32(world, "_minlight").unwrap_or(0.0);
    SourceMapLighting {
        ambient: (color * (ambient / Q3MAP_LIGHTMAP_BYTE_SCALE)).to_array(),
        minlight: (color * (minlight / Q3MAP_LIGHTMAP_BYTE_SCALE)).to_array(),
    }
}

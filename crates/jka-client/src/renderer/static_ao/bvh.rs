//! Static ao bvh.
use crate::renderer::{
    scene, thread, Arc, BTreeMap, PathBuf, StaticAoJobKey, StaticAoRenderTriangle,
    StaticAoTexelSample, StaticAoTraceTriangle, StaticAoWorldSource, Vec2, Vec3,
    STATIC_AO_BVH_LEAF_TRIANGLES, STATIC_AO_HQ_BROAD_RADIUS, STATIC_AO_HQ_BROAD_WEIGHT,
    STATIC_AO_HQ_CONTACT_RADIUS, STATIC_AO_HQ_CONTACT_WEIGHT, STATIC_AO_HQ_MEDIUM_RADIUS,
    STATIC_AO_HQ_MEDIUM_WEIGHT, STATIC_AO_LIGHTMAP_BIAS, STATIC_AO_VERTEX_BIAS,
};

#[derive(Clone, Copy)]
pub(in crate::renderer) struct StaticAoBvhNode {
    pub(in crate::renderer) minimum: [f32; 3],
    pub(in crate::renderer) maximum: [f32; 3],
    pub(in crate::renderer) left: u32,
    pub(in crate::renderer) right: u32,
    pub(in crate::renderer) start: u32,
    pub(in crate::renderer) count: u32,
}

pub(in crate::renderer) struct StaticAoBvh {
    pub(in crate::renderer) triangles: Vec<StaticAoTraceTriangle>,
    pub(in crate::renderer) indices: Vec<u32>,
    pub(in crate::renderer) nodes: Vec<StaticAoBvhNode>,
}

pub(in crate::renderer) fn static_ao_cache_path(
    cache: &scene::StaticBspAoCacheInfo,
    key: StaticAoJobKey,
) -> PathBuf {
    let safe_name = cache
        .map_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    cache.directory.join(format!(
        "{safe_name}-{:016x}-v{}-{}-{}s-{}x-{}pct-{}range{}.ao8",
        cache.map_hash,
        cache.version,
        key.mode.label(),
        key.samples,
        key.scale,
        key.strength,
        key.range,
        if key.current_cell_only { "-cell" } else { "" }
    ))
}

pub(in crate::renderer) fn static_ao_rotation(seed: usize) -> f32 {
    let mut x = seed as u32;
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x as f32 / u32::MAX as f32) * std::f32::consts::TAU
}

pub(in crate::renderer) fn cross2(a: Vec2, b: Vec2) -> f32 {
    a.x * b.y - a.y * b.x
}

pub(in crate::renderer) fn rasterize_static_ao_texels(
    source: &StaticAoWorldSource,
    scale: u32,
) -> Vec<Vec<Option<StaticAoTexelSample>>> {
    let scale = scale.max(1);
    let mut texels = source
        .lightmap_sizes
        .iter()
        .map(|&[width, height]| {
            let width = width.saturating_mul(scale);
            let height = height.saturating_mul(scale);
            vec![None; width as usize * height as usize]
        })
        .collect::<Vec<_>>();

    for triangle in source.lightmap_triangles.iter() {
        let Some(&[base_width, base_height]) = source.lightmap_sizes.get(triangle.texture) else {
            continue;
        };
        let width = base_width.saturating_mul(scale);
        let height = base_height.saturating_mul(scale);
        if width == 0 || height == 0 {
            continue;
        }
        let size = Vec2::new(width as f32, height as f32);
        let p = triangle
            .uvs
            .map(|uv| Vec2::from_array(uv) * size - Vec2::splat(0.5));
        let denominator = cross2(p[1] - p[0], p[2] - p[0]);
        if denominator.abs() < 1.0e-8 {
            continue;
        }
        let min = p[0].min(p[1]).min(p[2]).floor();
        let max = p[0].max(p[1]).max(p[2]).ceil();
        let min_x = min.x.max(0.0) as u32;
        let min_y = min.y.max(0.0) as u32;
        let max_x = max.x.min(width.saturating_sub(1) as f32) as u32;
        let max_y = max.y.min(height.saturating_sub(1) as f32) as u32;

        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let q = Vec2::new(x as f32, y as f32);
                let w1 = cross2(q - p[0], p[2] - p[0]) / denominator;
                let w2 = cross2(p[1] - p[0], q - p[0]) / denominator;
                let w0 = 1.0 - w1 - w2;
                if w0 < -0.001 || w1 < -0.001 || w2 < -0.001 {
                    continue;
                }
                let position = Vec3::from_array(triangle.positions[0]) * w0
                    + Vec3::from_array(triangle.positions[1]) * w1
                    + Vec3::from_array(triangle.positions[2]) * w2;
                let normal = (Vec3::from_array(triangle.normals[0]) * w0
                    + Vec3::from_array(triangle.normals[1]) * w1
                    + Vec3::from_array(triangle.normals[2]) * w2)
                    .normalize_or_zero();
                if normal.length_squared() < 0.5 {
                    continue;
                }
                let index = y as usize * width as usize + x as usize;
                if texels[triangle.texture][index].is_none() {
                    texels[triangle.texture][index] = Some(StaticAoTexelSample {
                        position: position.to_array(),
                        normal: normal.to_array(),
                    });
                }
            }
        }
    }
    texels
}

pub(in crate::renderer) fn rasterize_static_ao_owners(
    source: &StaticAoWorldSource,
    scale: u32,
) -> Vec<Vec<u32>> {
    let scale = scale.max(1);
    let mut owners = source
        .lightmap_sizes
        .iter()
        .map(|&[width, height]| {
            let width = width.saturating_mul(scale);
            let height = height.saturating_mul(scale);
            vec![0u32; width as usize * height as usize]
        })
        .collect::<Vec<_>>();

    for triangle in source.lightmap_triangles.iter() {
        let Some(&[base_width, base_height]) = source.lightmap_sizes.get(triangle.texture) else {
            continue;
        };
        let width = base_width.saturating_mul(scale);
        let height = base_height.saturating_mul(scale);
        if width == 0 || height == 0 {
            continue;
        }
        let size = Vec2::new(width as f32, height as f32);
        let p = triangle
            .uvs
            .map(|uv| Vec2::from_array(uv) * size - Vec2::splat(0.5));
        let denominator = cross2(p[1] - p[0], p[2] - p[0]);
        if denominator.abs() < 1.0e-8 {
            continue;
        }
        let min = p[0].min(p[1]).min(p[2]).floor();
        let max = p[0].max(p[1]).max(p[2]).ceil();
        let min_x = min.x.max(0.0) as u32;
        let min_y = min.y.max(0.0) as u32;
        let max_x = max.x.min(width.saturating_sub(1) as f32) as u32;
        let max_y = max.y.min(height.saturating_sub(1) as f32) as u32;

        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let q = Vec2::new(x as f32, y as f32);
                let w1 = cross2(q - p[0], p[2] - p[0]) / denominator;
                let w2 = cross2(p[1] - p[0], q - p[0]) / denominator;
                let w0 = 1.0 - w1 - w2;
                if w0 < -0.001 || w1 < -0.001 || w2 < -0.001 {
                    continue;
                }
                let index = y as usize * width as usize + x as usize;
                if owners[triangle.texture][index] == 0 {
                    owners[triangle.texture][index] = triangle.owner;
                }
            }
        }
    }
    owners
}

pub(in crate::renderer) fn dilate_static_ao_owners(
    owners: &mut [u32],
    width: u32,
    height: u32,
    steps: u32,
) {
    for _ in 0..steps.max(1) {
        let previous = owners.to_vec();
        let mut changed = false;
        for y in 0..height as i32 {
            for x in 0..width as i32 {
                let index = y as usize * width as usize + x as usize;
                if previous[index] != 0 {
                    continue;
                }
                let mut chosen = 0u32;
                let mut count = 0u32;
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                        continue;
                    }
                    let neighbor = ny as usize * width as usize + nx as usize;
                    let owner = previous[neighbor];
                    if owner == 0 {
                        continue;
                    }
                    if count == 0 {
                        chosen = owner;
                    }
                    if owner == chosen {
                        count += 1;
                    } else if count == 1 {
                        chosen = owner;
                    }
                }
                if chosen != 0 {
                    owners[index] = chosen;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

pub(in crate::renderer) fn dilate_static_ao_mask(
    mask: &mut [u8],
    covered: &mut [u8],
    width: u32,
    height: u32,
    scale: u32,
) {
    // Preserve the atlas/page padding at the AO supersampling scale. Two native
    // texels of dilation becomes 2*scale high-resolution texels.
    for _ in 0..(2 * scale.max(1)) {
        let previous_mask = mask.to_vec();
        let previous_covered = covered.to_vec();
        let mut changed = false;
        for y in 0..height as i32 {
            for x in 0..width as i32 {
                let index = y as usize * width as usize + x as usize;
                if previous_covered[index] != 0 {
                    continue;
                }
                let mut total = 0u32;
                let mut count = 0u32;
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                        continue;
                    }
                    let neighbor = ny as usize * width as usize + nx as usize;
                    if previous_covered[neighbor] != 0 {
                        total += u32::from(previous_mask[neighbor]);
                        count += 1;
                    }
                }
                if count != 0 {
                    mask[index] = (total / count) as u8;
                    covered[index] = 1;
                    changed = true;
                }
            }
        }
        if !changed {
            break;
        }
    }
}

pub(in crate::renderer) fn spread_static_ao_mask(
    mask: &mut [u8],
    covered: &[u8],
    owners: &[u32],
    width: u32,
    height: u32,
    scale: u32,
) {
    // A deliberately conservative post-bake shoulder: only already-dark AO may
    // spread, only across covered texels belonging to the same lightmap owner,
    // and it fades to roughly 25% strength after about 1.25 native lightmap
    // texels. This slightly fatter shoulder helps contact shadows read a bit
    // farther from the seam without reverting to broad room-scale darkening.
    // This fattens genuine contact shadows without reintroducing the old broad
    // room/floor darkening from large AO radii.
    let radius = (scale.max(1).saturating_mul(5) + 3) / 4;
    let step_decay = 0.25f32.powf(1.0 / radius as f32);
    let mut occlusion = mask
        .iter()
        .map(|&value| 255u8.saturating_sub(value))
        .collect::<Vec<_>>();
    for _ in 0..radius {
        let previous = occlusion.clone();
        for y in 0..height as i32 {
            for x in 0..width as i32 {
                let index = y as usize * width as usize + x as usize;
                if covered[index] == 0 || owners[index] == 0 {
                    continue;
                }
                let owner = owners[index];
                let mut best = f32::from(previous[index]);
                for (dx, dy) in [
                    (-1, -1),
                    (0, -1),
                    (1, -1),
                    (-1, 0),
                    (1, 0),
                    (-1, 1),
                    (0, 1),
                    (1, 1),
                ] {
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                        continue;
                    }
                    let neighbor = ny as usize * width as usize + nx as usize;
                    if covered[neighbor] == 0 || owners[neighbor] != owner {
                        continue;
                    }
                    best = best.max(f32::from(previous[neighbor]) * step_decay);
                }
                occlusion[index] = best.clamp(0.0, 255.0).round() as u8;
            }
        }
    }
    for (visibility, &darkness) in mask.iter_mut().zip(occlusion.iter()) {
        *visibility = 255u8.saturating_sub(darkness);
    }
}

pub(in crate::renderer) fn static_ao_trace_triangle(
    triangle: StaticAoRenderTriangle,
) -> StaticAoTraceTriangle {
    let origin = Vec3::from_array(triangle.positions[0]);
    let b = Vec3::from_array(triangle.positions[1]);
    let c = Vec3::from_array(triangle.positions[2]);
    let edge1 = b - origin;
    let edge2 = c - origin;
    let minimum = origin.min(b).min(c);
    let maximum = origin.max(b).max(c);
    StaticAoTraceTriangle {
        origin,
        edge1,
        edge2,
        minimum,
        maximum,
        centroid: (origin + b + c) / 3.0,
    }
}

impl StaticAoBvh {
    pub(in crate::renderer) fn build(
        triangles: Arc<Vec<StaticAoRenderTriangle>>,
    ) -> Result<Self, String> {
        if triangles.is_empty() {
            return Err("map has no opaque render triangles for HQ AO".into());
        }
        // Convert once into the exact representation used by the hot ray loop.
        // This avoids rebuilding triangle edges / Vec3 values for every ray hit test.
        let triangles = triangles
            .iter()
            .copied()
            .map(static_ao_trace_triangle)
            .collect::<Vec<_>>();
        let mut indices = (0..triangles.len() as u32).collect::<Vec<_>>();
        let mut nodes = Vec::with_capacity(triangles.len().saturating_mul(2));
        Self::build_node(&triangles, &mut indices, &mut nodes, 0, triangles.len());
        Ok(Self {
            triangles,
            indices,
            nodes,
        })
    }

    pub(in crate::renderer) fn build_node(
        triangles: &[StaticAoTraceTriangle],
        indices: &mut [u32],
        nodes: &mut Vec<StaticAoBvhNode>,
        start: usize,
        end: usize,
    ) -> u32 {
        let mut minimum = Vec3::splat(f32::INFINITY);
        let mut maximum = Vec3::splat(f32::NEG_INFINITY);
        let mut centroid_min = Vec3::splat(f32::INFINITY);
        let mut centroid_max = Vec3::splat(f32::NEG_INFINITY);
        for &index in &indices[start..end] {
            let triangle = triangles[index as usize];
            minimum = minimum.min(triangle.minimum);
            maximum = maximum.max(triangle.maximum);
            centroid_min = centroid_min.min(triangle.centroid);
            centroid_max = centroid_max.max(triangle.centroid);
        }
        let node_index = nodes.len() as u32;
        nodes.push(StaticAoBvhNode {
            minimum: minimum.to_array(),
            maximum: maximum.to_array(),
            left: u32::MAX,
            right: u32::MAX,
            start: start as u32,
            count: (end - start) as u32,
        });
        if end - start <= STATIC_AO_BVH_LEAF_TRIANGLES {
            return node_index;
        }
        let extent = centroid_max - centroid_min;
        let axis = if extent.x >= extent.y && extent.x >= extent.z {
            0
        } else if extent.y >= extent.z {
            1
        } else {
            2
        };
        indices[start..end].sort_unstable_by(|&a, &b| {
            triangles[a as usize].centroid[axis].total_cmp(&triangles[b as usize].centroid[axis])
        });
        let middle = start + (end - start) / 2;
        let left = Self::build_node(triangles, indices, nodes, start, middle);
        let right = Self::build_node(triangles, indices, nodes, middle, end);
        nodes[node_index as usize].left = left;
        nodes[node_index as usize].right = right;
        nodes[node_index as usize].count = 0;
        node_index
    }

    pub(in crate::renderer) fn ray_bounds_near(
        origin: Vec3,
        direction: Vec3,
        inverse_direction: Vec3,
        minimum: Vec3,
        maximum: Vec3,
        max_t: f32,
    ) -> Option<f32> {
        let mut near = 0.0f32;
        let mut far = max_t;
        for axis in 0..3 {
            if direction[axis].abs() < 1.0e-8 {
                if origin[axis] < minimum[axis] || origin[axis] > maximum[axis] {
                    return None;
                }
                continue;
            }
            let mut t0 = (minimum[axis] - origin[axis]) * inverse_direction[axis];
            let mut t1 = (maximum[axis] - origin[axis]) * inverse_direction[axis];
            if t0 > t1 {
                std::mem::swap(&mut t0, &mut t1);
            }
            near = near.max(t0);
            far = far.min(t1);
            if far < near {
                return None;
            }
        }
        (far >= 0.0 && near <= max_t).then_some(near.max(0.0))
    }

    #[inline]
    pub(in crate::renderer) fn ray_triangle_distance(
        origin: Vec3,
        direction: Vec3,
        triangle: StaticAoTraceTriangle,
    ) -> Option<f32> {
        let p = direction.cross(triangle.edge2);
        let determinant = triangle.edge1.dot(p);
        if determinant.abs() < 1.0e-7 {
            return None;
        }
        let inverse = 1.0 / determinant;
        let t = origin - triangle.origin;
        let u = t.dot(p) * inverse;
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let q = t.cross(triangle.edge1);
        let v = direction.dot(q) * inverse;
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        let distance = triangle.edge2.dot(q) * inverse;
        (distance > 0.001 && distance.is_finite()).then_some(distance)
    }

    pub(in crate::renderer) fn trace(
        &self,
        origin: Vec3,
        direction: Vec3,
        max_distance: f32,
    ) -> Option<f32> {
        let inverse_direction = Vec3::new(
            if direction.x.abs() >= 1.0e-8 {
                1.0 / direction.x
            } else {
                f32::INFINITY
            },
            if direction.y.abs() >= 1.0e-8 {
                1.0 / direction.y
            } else {
                f32::INFINITY
            },
            if direction.z.abs() >= 1.0e-8 {
                1.0 / direction.z
            } else {
                f32::INFINITY
            },
        );
        let root = self.nodes[0];
        let root_near = Self::ray_bounds_near(
            origin,
            direction,
            inverse_direction,
            Vec3::from_array(root.minimum),
            Vec3::from_array(root.maximum),
            max_distance,
        )?;

        let mut closest = max_distance;
        let mut hit = false;
        // Near-first traversal lets a close hit tighten `closest` before we visit
        // the far branch, pruning a large amount of work in dense BSP scenes.
        let mut stack = [(0_u32, 0.0_f32); 64];
        let mut stack_len = 1usize;
        stack[0] = (0, root_near);
        while stack_len != 0 {
            stack_len -= 1;
            let (node_index, node_near) = stack[stack_len];
            if node_near > closest {
                continue;
            }
            let node = self.nodes[node_index as usize];
            if node.count != 0 {
                let start = node.start as usize;
                let end = start + node.count as usize;
                for &triangle_index in &self.indices[start..end] {
                    let triangle = self.triangles[triangle_index as usize];
                    if let Some(distance) = Self::ray_triangle_distance(origin, direction, triangle)
                    {
                        if distance < closest {
                            closest = distance;
                            hit = true;
                        }
                    }
                }
                continue;
            }

            let left = self.nodes[node.left as usize];
            let right = self.nodes[node.right as usize];
            let left_near = Self::ray_bounds_near(
                origin,
                direction,
                inverse_direction,
                Vec3::from_array(left.minimum),
                Vec3::from_array(left.maximum),
                closest,
            );
            let right_near = Self::ray_bounds_near(
                origin,
                direction,
                inverse_direction,
                Vec3::from_array(right.minimum),
                Vec3::from_array(right.maximum),
                closest,
            );
            if stack_len + 2 > stack.len() {
                break;
            }
            match (left_near, right_near) {
                (Some(left_t), Some(right_t)) if left_t <= right_t => {
                    stack[stack_len] = (node.right, right_t);
                    stack[stack_len + 1] = (node.left, left_t);
                    stack_len += 2;
                }
                (Some(left_t), Some(right_t)) => {
                    stack[stack_len] = (node.left, left_t);
                    stack[stack_len + 1] = (node.right, right_t);
                    stack_len += 2;
                }
                (Some(left_t), None) => {
                    stack[stack_len] = (node.left, left_t);
                    stack_len += 1;
                }
                (None, Some(right_t)) => {
                    stack[stack_len] = (node.right, right_t);
                    stack_len += 1;
                }
                (None, None) => {}
            }
        }
        hit.then_some(closest)
    }
}

pub(in crate::renderer) fn static_ao_sample_pattern(samples: u32) -> Vec<[f32; 2]> {
    let samples = samples.clamp(1, 128);
    (0..samples)
        .map(|sample| {
            let u = (sample as f32 + 0.5) / samples as f32;
            [u.sqrt(), (1.0 - u).sqrt()]
        })
        .collect()
}

pub(in crate::renderer) fn static_ao_hq_visibility(
    bvh: &StaticAoBvh,
    render_position: [f32; 3],
    render_normal: [f32; 3],
    sample_seed: usize,
    sample_pattern: &[[f32; 2]],
    range_scale: f32,
) -> u8 {
    let position = Vec3::from_array(render_position);
    let normal = Vec3::from_array(render_normal).normalize_or_zero();
    if normal.length_squared() < 0.5 || sample_pattern.is_empty() {
        return 255;
    }
    let helper = if normal.z.abs() < 0.9 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let tangent = normal.cross(helper).normalize_or_zero();
    let bitangent = normal.cross(tangent).normalize_or_zero();
    let rotation = static_ao_rotation(sample_seed);
    let start = position + normal * STATIC_AO_LIGHTMAP_BIAS;
    const GOLDEN_COS: f32 = -0.737_368_8;
    const GOLDEN_SIN: f32 = 0.675_490_4;
    let (mut sin_phi, mut cos_phi) = rotation.sin_cos();
    let contact_radius = STATIC_AO_HQ_CONTACT_RADIUS * range_scale;
    let medium_radius = STATIC_AO_HQ_MEDIUM_RADIUS * range_scale;
    let broad_radius = STATIC_AO_HQ_BROAD_RADIUS * range_scale;
    let mut contact = 0.0f32;
    let mut medium = 0.0f32;
    let mut broad = 0.0f32;
    for &[radial, z] in sample_pattern {
        let direction = tangent * (radial * cos_phi) + bitangent * (radial * sin_phi) + normal * z;
        let next_cos = cos_phi * GOLDEN_COS - sin_phi * GOLDEN_SIN;
        let next_sin = sin_phi * GOLDEN_COS + cos_phi * GOLDEN_SIN;
        cos_phi = next_cos;
        sin_phi = next_sin;
        let Some(distance) = bvh.trace(start, direction, broad_radius) else {
            continue;
        };
        let contribution = |radius: f32| {
            if distance >= radius {
                0.0
            } else {
                let proximity = 1.0 - distance / radius;
                proximity * proximity
            }
        };
        contact += contribution(contact_radius);
        medium += contribution(medium_radius);
        broad += contribution(broad_radius);
    }
    let inv = 1.0 / sample_pattern.len() as f32;
    let occlusion = STATIC_AO_HQ_CONTACT_WEIGHT * contact * inv
        + STATIC_AO_HQ_MEDIUM_WEIGHT * medium * inv
        + STATIC_AO_HQ_BROAD_WEIGHT * broad * inv;
    let visibility = (1.0 - occlusion).clamp(0.0, 1.0);
    (visibility * 255.0 + 0.5) as u8
}

pub(in crate::renderer) fn static_ao_structural_visibility(
    bvh: &StaticAoBvh,
    render_position: [f32; 3],
    render_normal: [f32; 3],
    sample_seed: usize,
    sample_pattern: &[[f32; 2]],
    range_scale: f32,
) -> u8 {
    let position = Vec3::from_array(render_position);
    let normal = Vec3::from_array(render_normal).normalize_or_zero();
    if normal.length_squared() < 0.5 || sample_pattern.is_empty() {
        return 255;
    }
    let helper = if normal.z.abs() < 0.9 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let tangent = normal.cross(helper).normalize_or_zero();
    let bitangent = normal.cross(tangent).normalize_or_zero();
    let rotation = static_ao_rotation(sample_seed);
    let start = position + normal * STATIC_AO_VERTEX_BIAS;
    let radius = STATIC_AO_HQ_MEDIUM_RADIUS * range_scale;
    const GOLDEN_COS: f32 = -0.737_368_8;
    const GOLDEN_SIN: f32 = 0.675_490_4;
    let (mut sin_phi, mut cos_phi) = rotation.sin_cos();
    let mut occlusion = 0.0f32;

    for &[radial, z] in sample_pattern {
        let direction = tangent * (radial * cos_phi) + bitangent * (radial * sin_phi) + normal * z;
        let next_cos = cos_phi * GOLDEN_COS - sin_phi * GOLDEN_SIN;
        let next_sin = sin_phi * GOLDEN_COS + cos_phi * GOLDEN_SIN;
        cos_phi = next_cos;
        sin_phi = next_sin;
        let Some(distance) = bvh.trace(start, direction, radius) else {
            continue;
        };
        let proximity = (1.0 - distance / radius).clamp(0.0, 1.0);
        occlusion += proximity * proximity;
    }

    let visibility = (1.0 - occlusion / sample_pattern.len() as f32).clamp(0.0, 1.0);
    (visibility * 255.0 + 0.5) as u8
}

pub(in crate::renderer) fn static_ao_worker_count() -> usize {
    let logical_workers = thread::available_parallelism().map_or(4, |count| count.get());
    // The bake is asynchronous and cacheable. Use most of the CPU, but leave one
    // logical core for the render/game threads so enabling BAKED remains responsive.
    logical_workers.saturating_sub(1).clamp(1, 16)
}

pub(in crate::renderer) fn bake_static_structural_vertex_ao(
    source: &StaticAoWorldSource,
    bvh: Arc<StaticAoBvh>,
    sample_pattern: Arc<Vec<[f32; 2]>>,
    range_scale: f32,
    workers: usize,
) -> Vec<u8> {
    let mut values = vec![255u8; source.vertices.len()];
    // Vertex AO is only for true LIGHTMAP_BY_VERTEX/static-model receivers.
    // Lightmapped BSP is handled exclusively in lightmap space so large BSP
    // polygons cannot smear one dark vertex across a huge floor or wall.
    let mut unique_lookup = BTreeMap::<[u32; 6], usize>::new();
    let mut unique_vertices = Vec::<([f32; 3], [f32; 3], usize)>::new();
    let mut vertex_to_unique = vec![usize::MAX; source.vertices.len()];
    for (index, vertex) in source.vertices.iter().enumerate() {
        if source.vertex_ao_receivers.get(index).copied().unwrap_or(0) == 0 {
            continue;
        }
        let key = [
            vertex.position[0].to_bits(),
            vertex.position[1].to_bits(),
            vertex.position[2].to_bits(),
            vertex.normal[0].to_bits(),
            vertex.normal[1].to_bits(),
            vertex.normal[2].to_bits(),
        ];
        let unique_index = if let Some(&unique_index) = unique_lookup.get(&key) {
            unique_index
        } else {
            let unique_index = unique_vertices.len();
            unique_lookup.insert(key, unique_index);
            unique_vertices.push((vertex.position, vertex.normal, index));
            unique_index
        };
        vertex_to_unique[index] = unique_index;
    }

    let mut unique_values = vec![255u8; unique_vertices.len()];
    let chunk_size = unique_values.len().div_ceil(workers).max(1);
    thread::scope(|scope| {
        for (value_chunk, vertex_chunk) in unique_values
            .chunks_mut(chunk_size)
            .zip(unique_vertices.chunks(chunk_size))
        {
            let bvh = Arc::clone(&bvh);
            let sample_pattern = Arc::clone(&sample_pattern);
            scope.spawn(move || {
                for (visibility, &(position, normal, seed)) in
                    value_chunk.iter_mut().zip(vertex_chunk.iter())
                {
                    *visibility = static_ao_structural_visibility(
                        &bvh,
                        position,
                        normal,
                        seed,
                        &sample_pattern,
                        range_scale,
                    );
                }
            });
        }
    });

    for (index, &unique_index) in vertex_to_unique.iter().enumerate() {
        if unique_index != usize::MAX {
            values[index] = unique_values[unique_index];
        }
    }
    rverbose!(
        1,
        "Static BSP AO worker: vertex-lit AO baked {} receiver vertices / {} unique x {} rays ({} threads)",
        vertex_to_unique.iter().filter(|&&index| index != usize::MAX).count(),
        unique_vertices.len(),
        sample_pattern.len(),
        workers
    );
    values
}

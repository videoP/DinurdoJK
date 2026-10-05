//! Lighting legacy dlights.
use crate::renderer::{scene, InspectorVertex, TransientLight, Vec3};
use crate::renderer::{DrawBatch, LegacyDlightRun};

pub(in crate::renderer) fn ray_hits_aabb(
    origin: Vec3,
    direction: Vec3,
    minimum: Vec3,
    maximum: Vec3,
) -> bool {
    let mut t_min = 0.0_f32;
    let mut t_max = f32::INFINITY;
    for axis in 0..3 {
        let o = origin[axis];
        let d = direction[axis];
        let min = minimum[axis];
        let max = maximum[axis];
        if d.abs() < 1e-8 {
            if o < min || o > max {
                return false;
            }
            continue;
        }
        let inv = 1.0 / d;
        let mut a = (min - o) * inv;
        let mut b = (max - o) * inv;
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        t_min = t_min.max(a);
        t_max = t_max.min(b);
        if t_max < t_min {
            return false;
        }
    }
    t_max >= 0.0
}

pub(in crate::renderer) fn barycentric_coordinates(
    point: Vec3,
    a: Vec3,
    b: Vec3,
    c: Vec3,
) -> Option<[f32; 3]> {
    let v0 = b - a;
    let v1 = c - a;
    let v2 = point - a;
    let d00 = v0.dot(v0);
    let d01 = v0.dot(v1);
    let d11 = v1.dot(v1);
    let d20 = v2.dot(v0);
    let d21 = v2.dot(v1);
    let denominator = d00 * d11 - d01 * d01;
    if denominator.abs() <= 1e-10 {
        return None;
    }
    let v = (d11 * d20 - d01 * d21) / denominator;
    let w = (d00 * d21 - d01 * d20) / denominator;
    let u = 1.0 - v - w;
    Some([u, v, w])
}

pub(in crate::renderer) fn vertex_dlight_cpu(
    position: Vec3,
    normal: Vec3,
    lights: &[TransientLight],
) -> Vec3 {
    let normal = normal.normalize_or_zero();
    let mut result = Vec3::ZERO;
    for light in lights.iter().take(32) {
        let light_position = Vec3::from_array(light.position);
        let delta = light_position - position;
        let distance_to_light = delta.length();
        let radius = light.radius.max(1.0);
        if distance_to_light <= 0.0001 {
            continue;
        }
        let normalized_distance = distance_to_light / radius;
        if normalized_distance >= 4.0 {
            continue;
        }
        let core_falloff = (1.0 - normalized_distance).max(0.0);
        let support_falloff = (1.0 - normalized_distance / 4.0).max(0.0);
        let attenuation =
            (core_falloff * core_falloff).max(support_falloff * support_falloff * 0.40);
        let light_direction = delta / distance_to_light;
        let ndotl = normal.dot(light_direction).max(0.0);
        let facing = 0.35 + 0.65 * ndotl;
        result +=
            Vec3::from_array(light.color) * light.intensity.max(0.0) * attenuation * facing * 0.70;
    }
    result
}

pub(in crate::renderer) fn legacy_dlight_surface_mask(
    surface: &scene::LegacyDlightSurface,
    lights: &[TransientLight],
    transient_count: usize,
) -> u32 {
    let transient_count = transient_count.min(32).min(lights.len());
    if transient_count == 0 || surface.cull_kind == 0 {
        return 0;
    }

    let minimum = Vec3::from_array(surface.bounds_min);
    let maximum = Vec3::from_array(surface.bounds_max);
    let normal = Vec3::new(surface.plane[0], surface.plane[1], surface.plane[2]);
    let planar = surface.cull_kind == 1 && normal.length_squared() > 1.0e-10;
    let mut bits = 0u32;

    for (light_index, light) in lights.iter().take(transient_count).enumerate() {
        let position = Vec3::from_array(light.position);
        let radius = light.radius.max(0.0);

        // OpenJK reaches R_DlightSurface with dlightBits already reduced by BSP
        // traversal. Preserve that early spatial rejection in the batched WGPU
        // path with a conservative surface-bounds test. Like R_DlightGrid, this
        // is radius-box overlap rather than a tighter sphere/AABB distance test.
        if position.x - radius > maximum.x
            || position.x + radius < minimum.x
            || position.y - radius > maximum.y
            || position.y + radius < minimum.y
            || position.z - radius > maximum.z
            || position.z + radius < minimum.z
        {
            continue;
        }

        // OpenJK R_DlightFace then rejects planar faces whose plane lies wholly
        // outside the light radius. Patch/triangle surfaces remain conservative
        // after the bounds test, matching the source renderer's intent.
        if planar {
            let distance = (normal.dot(position) - surface.plane[3]).abs();
            if distance > radius {
                continue;
            }
        }

        bits |= 1u32 << light_index;
    }

    bits
}

#[derive(Debug, Clone, Copy)]
pub(in crate::renderer) struct LegacyDlightDiagnostic {
    pub(in crate::renderer) plane_distance: f32,
    pub(in crate::renderer) projected_radius: f32,
    pub(in crate::renderer) radial_sq: f32,
    pub(in crate::renderer) blob: f32,
    pub(in crate::renderer) plane_modulate: f32,
}

/// CPU mirror of the Legacy shader path for the surface inspector.
pub(in crate::renderer) fn legacy_dlight_diagnostic(
    triangle: &[InspectorVertex],
    hit: Vec3,
    light: &TransientLight,
) -> Option<LegacyDlightDiagnostic> {
    if triangle.len() != 3 {
        return None;
    }
    let a = Vec3::from_array(triangle[0].position);
    let b = Vec3::from_array(triangle[1].position);
    let c = Vec3::from_array(triangle[2].position);
    let mut plane_normal = (b - a).cross(c - a).normalize_or_zero();
    if plane_normal.length_squared() <= 1.0e-10 {
        return None;
    }
    let authored_normal = (Vec3::from_array(triangle[0].normal)
        + Vec3::from_array(triangle[1].normal)
        + Vec3::from_array(triangle[2].normal))
    .normalize_or_zero();
    if authored_normal.length_squared() > 1.0e-8 && plane_normal.dot(authored_normal) < 0.0 {
        plane_normal = -plane_normal;
    }

    let radius = light.radius.max(1.0);
    let radius_sq = radius * radius;
    let delta = Vec3::from_array(light.position) - hit;
    let plane_distance = plane_normal.dot(delta);
    if plane_distance <= 0.0 || plane_distance >= radius {
        return Some(LegacyDlightDiagnostic {
            plane_distance,
            projected_radius: 0.0,
            radial_sq: f32::INFINITY,
            blob: 0.0,
            plane_modulate: 0.0,
        });
    }

    let projected_radius_sq = (radius_sq - plane_distance * plane_distance).max(1.0e-4);
    let planar_distance_sq = (delta.length_squared() - plane_distance * plane_distance).max(0.0);
    let radial_sq = planar_distance_sq / projected_radius_sq;
    let blob = if radial_sq < 1.0 {
        let x = 1.0 - radial_sq;
        x * x * (3.0 - 2.0 * x)
    } else {
        0.0
    };
    let plane_modulate = (1.0 - (plane_distance * plane_distance) / radius_sq).max(0.0);
    Some(LegacyDlightDiagnostic {
        plane_distance,
        projected_radius: projected_radius_sq.sqrt(),
        radial_sq,
        blob,
        plane_modulate,
    })
}

pub(in crate::renderer) fn legacy_dlight_runs_for_batch(
    source: &DrawBatch,
    indexed_range: &std::ops::Range<u32>,
    triangle_surfaces: Option<&[u32]>,
) -> Vec<LegacyDlightRun> {
    let Some(triangle_surfaces) = triangle_surfaces else {
        return Vec::new();
    };
    let source_start = source.vertices.start as usize;
    let source_count = source.vertices.end.saturating_sub(source.vertices.start) as usize;
    if source_count < 3
        || indexed_range.end.saturating_sub(indexed_range.start) as usize != source_count
    {
        return Vec::new();
    }

    let mut runs = Vec::<LegacyDlightRun>::new();
    for local_start in (0..source_count).step_by(3) {
        let source_vertex = source_start.saturating_add(local_start);
        let surface_id = triangle_surfaces
            .get(source_vertex / 3)
            .copied()
            .unwrap_or(u32::MAX);
        let first_index = indexed_range
            .start
            .saturating_add(u32::try_from(local_start).unwrap_or(u32::MAX));
        let end_index = first_index.saturating_add(3);
        if let Some(last) = runs.last_mut() {
            if last.surface_id == surface_id && last.indexed_range.end == first_index {
                last.indexed_range.end = end_index;
                continue;
            }
        }
        runs.push(LegacyDlightRun {
            surface_id,
            indexed_range: first_index..end_index,
        });
    }
    runs
}

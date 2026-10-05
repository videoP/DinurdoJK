//! Reflections planar.
use crate::renderer::{
    aabb_intersects_clip_frustum, batch_area_visible, effective_area_mask, scene, Camera, CullMode,
    GpuVertex, Mat4, PlanarReflectionMode, PlanarReflectionResources, PlanarReflectionUniform,
    PlanarReflectionView, PlanarReflector, PlanarReflectorKind, PvsMode, ReflectionQuality,
    Renderer, Vec2, Vec3, Vec4, WorldBatch, WorldGpu, DEPTH_FORMAT, PLANAR_AUTHORED_SCALE_DIVISOR,
    PLANAR_ENVIRONMENT_SCALE_DIVISOR, PLANAR_REFLECTION_HYSTERESIS, PLANAR_REFLECTION_SLOTS,
};
use wgpu::util::DeviceExt;

impl Renderer {
    pub(in crate::renderer) fn rebuild_planar_reflection_resources(&mut self) {
        let planar_budget = self
            .reflection_quality
            .planar_slot_budget()
            .min(PLANAR_REFLECTION_SLOTS);
        let active = planar_budget > 0
            && self.world.as_ref().is_some_and(|world| {
                world.planar_reflectors.iter().any(|reflector| {
                    (reflector.kind != PlanarReflectorKind::Ocean || self.ocean_enabled)
                        && planar_reflector_enabled(self.planar_reflection_mode, reflector.kind)
                })
            });
        // Promoted tcGen-environment surfaces are intentionally broad/rough
        // reflections, so an environment-only pool can be rendered at quarter
        // resolution. If a real authored mirror is active, keep the pool at
        // half resolution so the authored mirror remains crisp.
        let has_authored = self.world.as_ref().is_some_and(|world| {
            world.planar_reflectors.iter().any(|reflector| {
                reflector.kind == PlanarReflectorKind::Authored
                    && planar_reflector_enabled(self.planar_reflection_mode, reflector.kind)
            })
        });
        let scale_divisor = if has_authored {
            // A JKA-authored portal is a real mirror, so do not demote it to the
            // quarter-resolution environment-promotion pool at Legacy/Low/Medium.
            PLANAR_AUTHORED_SCALE_DIVISOR
        } else {
            match self.reflection_quality {
                ReflectionQuality::Ultra => PLANAR_AUTHORED_SCALE_DIVISOR,
                _ => PLANAR_ENVIRONMENT_SCALE_DIVISOR,
            }
        };
        let (width, height) = if active {
            (
                self.config.width.max(1).div_ceil(scale_divisor),
                self.config.height.max(1).div_ceil(scale_divisor),
            )
        } else {
            (1, 1)
        };
        self.planar_reflection = create_planar_reflection_resources(
            &self.device,
            &self.planar_reflection_layout,
            self.scene_format(),
            width,
            height,
            active,
        );
        self.planar_slot_history = [None; PLANAR_REFLECTION_SLOTS];
        self.last_planar_debug_selection = None;
        if active {
            let (authored, environment, ocean) = self.world.as_ref().map_or((0, 0, 0), |world| {
                world
                    .planar_reflectors
                    .iter()
                    .fold((0, 0, 0), |counts, reflector| match reflector.kind {
                        PlanarReflectorKind::Authored => (counts.0 + 1, counts.1, counts.2),
                        PlanarReflectorKind::Environment => (counts.0, counts.1 + 1, counts.2),
                        PlanarReflectorKind::Ocean => (counts.0, counts.1, counts.2 + 1),
                    })
            });
            rverbose!(
                1,
                "Planar reflections: {}x{} reflected view, {} slots, mode={} (authored={}, promoted environment={}, ocean={})",
                width,
                height,
                planar_budget,
                self.planar_reflection_mode.label(),
                authored,
                environment,
                ocean
            );
        }
    }
}

pub(in crate::renderer) fn planar_reflector_enabled(
    mode: PlanarReflectionMode,
    kind: PlanarReflectorKind,
) -> bool {
    match mode {
        PlanarReflectionMode::Off => false,
        PlanarReflectionMode::Authored => kind == PlanarReflectorKind::Authored,
        PlanarReflectionMode::Environment => true,
    }
}

pub(in crate::renderer) fn collect_planar_reflectors(
    batches: &[WorldBatch],
    vertices: &[GpuVertex],
) -> Vec<PlanarReflector> {
    let mut seen_ranges = std::collections::BTreeSet::new();
    let mut reflectors = Vec::new();
    for (batch_index, batch) in batches.iter().enumerate() {
        let kind = if batch.ocean_clipmap.is_some() {
            PlanarReflectorKind::Ocean
        } else if batch.source.planar_reflection {
            PlanarReflectorKind::Authored
        } else if batch.source.planar_environment_candidate {
            PlanarReflectorKind::Environment
        } else {
            continue;
        };
        if !seen_ranges.insert((batch.source.vertices.start, batch.source.vertices.end)) {
            continue;
        }
        let normal = Vec3::new(
            batch.source.planar_plane[0],
            batch.source.planar_plane[1],
            batch.source.planar_plane[2],
        );
        if normal.length_squared() <= 0.5 {
            continue;
        }
        let start = batch.source.vertices.start as usize;
        let end = batch.source.vertices.end as usize;
        let Some(slice) = vertices.get(start..end) else {
            continue;
        };
        let area = slice
            .chunks_exact(3)
            .map(|triangle| {
                let a = Vec3::from_array(triangle[0].position);
                let b = Vec3::from_array(triangle[1].position);
                let c = Vec3::from_array(triangle[2].position);
                0.5 * (b - a).cross(c - a).length()
            })
            .sum::<f32>();
        if !area.is_finite() || area <= 0.01 {
            continue;
        }
        let centre = [
            (batch.bounds_min[0] + batch.bounds_max[0]) * 0.5,
            (batch.bounds_min[1] + batch.bounds_max[1]) * 0.5,
            (batch.bounds_min[2] + batch.bounds_max[2]) * 0.5,
        ];
        reflectors.push(PlanarReflector {
            kind,
            plane: batch.source.planar_plane,
            // Authored JKA mirrors use misc_portal_surface exactly. Promoted
            // environment surfaces have no portal entity, so retain their
            // surface centre and nudge it to the camera side at selection time.
            pvs_origin: if kind == PlanarReflectorKind::Authored {
                batch.source.planar_pvs_origin
            } else {
                centre
            },
            cull: if kind == PlanarReflectorKind::Ocean {
                CullMode::None
            } else {
                batch.source.pipeline.cull
            },
            bounds_min: batch.bounds_min,
            bounds_max: batch.bounds_max,
            area,
            coarse_batch_index: batch_index,
        });
    }
    reflectors
}

pub(in crate::renderer) fn reflect_point_across_plane(point: Vec3, plane: [f32; 4]) -> Vec3 {
    let normal = Vec3::new(plane[0], plane[1], plane[2]);
    point - normal * (2.0 * (normal.dot(point) + plane[3]))
}

pub(in crate::renderer) fn reflection_matrix(plane: [f32; 4]) -> Mat4 {
    let normal = Vec3::new(plane[0], plane[1], plane[2]);
    let nx = normal.x;
    let ny = normal.y;
    let nz = normal.z;
    let d = plane[3];
    Mat4::from_cols(
        glam::Vec4::new(1.0 - 2.0 * nx * nx, -2.0 * nx * ny, -2.0 * nx * nz, 0.0),
        glam::Vec4::new(-2.0 * ny * nx, 1.0 - 2.0 * ny * ny, -2.0 * ny * nz, 0.0),
        glam::Vec4::new(-2.0 * nz * nx, -2.0 * nz * ny, 1.0 - 2.0 * nz * nz, 0.0),
        glam::Vec4::new(-2.0 * d * nx, -2.0 * d * ny, -2.0 * d * nz, 1.0),
    )
}

pub(in crate::renderer) fn push_unique_planar_point(
    points: &mut [Vec3; 6],
    point_count: &mut usize,
    point: Vec3,
    epsilon_squared: f32,
) {
    if !point.is_finite()
        || points[..*point_count]
            .iter()
            .any(|existing| existing.distance_squared(point) <= epsilon_squared)
        || *point_count >= points.len()
    {
        return;
    }
    points[*point_count] = point;
    *point_count += 1;
}

pub(in crate::renderer) fn planar_aabb_intersection_polygon(
    bounds_min: [f32; 3],
    bounds_max: [f32; 3],
    plane: [f32; 4],
) -> Option<([Vec3; 6], usize)> {
    let normal = Vec3::new(plane[0], plane[1], plane[2]);
    if !normal.is_finite() || normal.length_squared() <= 1e-8 {
        return None;
    }

    let corners = [
        Vec3::new(bounds_min[0], bounds_min[1], bounds_min[2]),
        Vec3::new(bounds_max[0], bounds_min[1], bounds_min[2]),
        Vec3::new(bounds_min[0], bounds_max[1], bounds_min[2]),
        Vec3::new(bounds_max[0], bounds_max[1], bounds_min[2]),
        Vec3::new(bounds_min[0], bounds_min[1], bounds_max[2]),
        Vec3::new(bounds_max[0], bounds_min[1], bounds_max[2]),
        Vec3::new(bounds_min[0], bounds_max[1], bounds_max[2]),
        Vec3::new(bounds_max[0], bounds_max[1], bounds_max[2]),
    ];
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (0, 2),
        (0, 4),
        (1, 3),
        (1, 5),
        (2, 3),
        (2, 6),
        (3, 7),
        (4, 5),
        (4, 6),
        (5, 7),
        (6, 7),
    ];

    let plane_distance = |point: Vec3| normal.dot(point) + plane[3];
    let distance_epsilon = normal.length().max(1.0) * 1e-4;
    let point_epsilon_squared = 1e-4_f32;
    let mut points = [Vec3::ZERO; 6];
    let mut point_count = 0usize;

    for (a_index, b_index) in EDGES {
        let a = corners[a_index];
        let b = corners[b_index];
        let a_distance = plane_distance(a);
        let b_distance = plane_distance(b);
        let a_on_plane = a_distance.abs() <= distance_epsilon;
        let b_on_plane = b_distance.abs() <= distance_epsilon;

        if a_on_plane {
            push_unique_planar_point(&mut points, &mut point_count, a, point_epsilon_squared);
        }
        if b_on_plane {
            push_unique_planar_point(&mut points, &mut point_count, b, point_epsilon_squared);
        }
        if !a_on_plane && !b_on_plane && (a_distance < 0.0) != (b_distance < 0.0) {
            let denominator = a_distance - b_distance;
            if denominator.abs() > f32::EPSILON {
                let t = (a_distance / denominator).clamp(0.0, 1.0);
                push_unique_planar_point(
                    &mut points,
                    &mut point_count,
                    a.lerp(b, t),
                    point_epsilon_squared,
                );
            }
        }
    }

    if point_count < 3 {
        return None;
    }

    // The edge intersections are not emitted in winding order. Sort them around
    // the plane centroid so homogeneous Sutherland-Hodgman clipping below sees
    // the actual convex planar polygon rather than a self-intersecting loop.
    let centroid = points[..point_count].iter().copied().sum::<Vec3>() / point_count as f32;
    let unit_normal = normal.normalize();
    let helper_axis = if unit_normal.x.abs() < 0.9 {
        Vec3::X
    } else {
        Vec3::Y
    };
    let tangent = unit_normal.cross(helper_axis).normalize_or_zero();
    let bitangent = unit_normal.cross(tangent).normalize_or_zero();
    if tangent.length_squared() <= 0.5 || bitangent.length_squared() <= 0.5 {
        return None;
    }
    points[..point_count].sort_unstable_by(|a, b| {
        let a_delta = *a - centroid;
        let b_delta = *b - centroid;
        let a_angle = a_delta.dot(bitangent).atan2(a_delta.dot(tangent));
        let b_angle = b_delta.dot(bitangent).atan2(b_delta.dot(tangent));
        a_angle.total_cmp(&b_angle)
    });
    Some((points, point_count))
}

pub(in crate::renderer) fn planar_clip_distance(point: Vec4, plane_index: usize) -> f32 {
    match plane_index {
        // Keep a small positive w before the perspective divide. This plane is
        // intentionally first so polygons that straddle the eye are clipped
        // rather than losing whichever distant AABB corners happen to be behind.
        0 => point.w - 1e-4,
        1 => point.x + point.w,
        2 => point.w - point.x,
        3 => point.y + point.w,
        4 => point.w - point.y,
        // glam's *_rh projection helpers use wgpu/D3D z in [0, w], including
        // our reversed-Z camera projection.
        5 => point.z,
        6 => point.w - point.z,
        _ => unreachable!(),
    }
}

pub(in crate::renderer) fn clip_planar_polygon_to_viewport(
    world_points: [Vec3; 6],
    world_point_count: usize,
    view_proj: Mat4,
) -> Option<([Vec4; 16], usize)> {
    // A convex N-gon can gain at most one vertex per clipping plane. A plane/AABB
    // intersection starts with at most six vertices, so 16 is comfortably above
    // the 13 vertices possible after the seven homogeneous clip planes.
    let mut current = [Vec4::ZERO; 16];
    let mut current_count = world_point_count.min(6);
    for (index, point) in world_points[..current_count].iter().enumerate() {
        current[index] = view_proj * point.extend(1.0);
    }

    for plane_index in 0..7 {
        if current_count < 3 {
            return None;
        }
        let mut next = [Vec4::ZERO; 16];
        let mut next_count = 0usize;
        let mut previous = current[current_count - 1];
        let mut previous_distance = planar_clip_distance(previous, plane_index);
        let mut previous_inside = previous_distance >= 0.0;

        for current_point in current[..current_count].iter().copied() {
            let current_distance = planar_clip_distance(current_point, plane_index);
            let current_inside = current_distance >= 0.0;
            if current_inside != previous_inside {
                let denominator = previous_distance - current_distance;
                if denominator.abs() > f32::EPSILON && next_count < next.len() {
                    let t = (previous_distance / denominator).clamp(0.0, 1.0);
                    next[next_count] = previous.lerp(current_point, t);
                    next_count += 1;
                }
            }
            if current_inside && next_count < next.len() {
                next[next_count] = current_point;
                next_count += 1;
            }
            previous = current_point;
            previous_distance = current_distance;
            previous_inside = current_inside;
        }

        current = next;
        current_count = next_count;
    }

    (current_count >= 3).then_some((current, current_count))
}

#[cfg(test)]
pub(in crate::renderer) fn projected_planar_importance(
    bounds_min: [f32; 3],
    bounds_max: [f32; 3],
    plane: [f32; 4],
    view_proj: Mat4,
) -> Option<(bool, f32)> {
    projected_planar_ndc_rect(bounds_min, bounds_max, plane, view_proj)
        .map(|(ndc_min, ndc_max)| planar_importance_from_ndc_rect(ndc_min, ndc_max))
}

/// Viewport-clipped NDC bounds of the reflector's plane polygon.
pub(in crate::renderer) fn projected_planar_ndc_rect(
    bounds_min: [f32; 3],
    bounds_max: [f32; 3],
    plane: [f32; 4],
    view_proj: Mat4,
) -> Option<(Vec2, Vec2)> {
    // AABB-corner projection is not sufficient for large floors/lakes: when the
    // camera is over the middle of the plane, every distant corner can be behind
    // the eye or off-screen even though the reflector fills the viewport. Build
    // the bounded plane polygon, clip it against the homogeneous view frustum,
    // then rank the portion that is actually visible.
    let (world_points, world_point_count) =
        planar_aabb_intersection_polygon(bounds_min, bounds_max, plane)?;
    let (visible_points, visible_point_count) =
        clip_planar_polygon_to_viewport(world_points, world_point_count, view_proj)?;

    let mut ndc_min = Vec2::splat(f32::INFINITY);
    let mut ndc_max = Vec2::splat(f32::NEG_INFINITY);
    for clip in visible_points[..visible_point_count].iter().copied() {
        if !clip.is_finite() || clip.w <= 1e-4 {
            continue;
        }
        let ndc = clip.truncate().truncate() / clip.w;
        if !ndc.is_finite() {
            continue;
        }
        ndc_min = ndc_min.min(ndc);
        ndc_max = ndc_max.max(ndc);
    }
    if !ndc_min.is_finite() || !ndc_max.is_finite() {
        return None;
    }

    ndc_min = ndc_min.max(Vec2::splat(-1.0));
    ndc_max = ndc_max.min(Vec2::splat(1.0));
    let extent = (ndc_max - ndc_min).max(Vec2::ZERO);
    if extent.x <= 0.0 || extent.y <= 0.0 {
        return None;
    }
    Some((ndc_min, ndc_max))
}

pub(in crate::renderer) fn planar_importance_from_ndc_rect(
    ndc_min: Vec2,
    ndc_max: Vec2,
) -> (bool, f32) {
    let extent = (ndc_max - ndc_min).max(Vec2::ZERO);
    // NDC spans two units per axis, so divide by four for a 0..1 viewport
    // coverage estimate. Prefer a reflector covering the crosshair/centre;
    // otherwise attenuate candidates toward the screen edge.
    let coverage = (extent.x * extent.y * 0.25).clamp(0.0, 1.0);
    let contains_centre =
        ndc_min.x <= 0.0 && ndc_max.x >= 0.0 && ndc_min.y <= 0.0 && ndc_max.y >= 0.0;
    let centre = (ndc_min + ndc_max) * 0.5;
    let centre_weight = 1.0 / (1.0 + centre.length_squared() * 3.0);
    (contains_centre, coverage * centre_weight)
}

/// Clip-space scale/offset that maps the NDC rectangle onto the full viewport,
/// so a frustum test against `narrow * view_proj` rejects anything outside it.
pub(in crate::renderer) fn ndc_rect_narrowing(ndc_min: Vec2, ndc_max: Vec2) -> Mat4 {
    // A small margin keeps edge pixels (and filtering) conservative.
    let ndc_min = (ndc_min - Vec2::splat(0.02)).max(Vec2::splat(-1.0));
    let ndc_max = (ndc_max + Vec2::splat(0.02)).min(Vec2::splat(1.0));
    let scale = Vec2::splat(2.0) / (ndc_max - ndc_min).max(Vec2::splat(1e-4));
    let centre = (ndc_min + ndc_max) * 0.5;
    Mat4::from_cols(
        glam::Vec4::new(scale.x, 0.0, 0.0, 0.0),
        glam::Vec4::new(0.0, scale.y, 0.0, 0.0),
        glam::Vec4::new(0.0, 0.0, 1.0, 0.0),
        glam::Vec4::new(-scale.x * centre.x, -scale.y * centre.y, 0.0, 1.0),
    )
}

pub(in crate::renderer) fn planar_planes_equivalent(a: [f32; 4], b: [f32; 4]) -> bool {
    let a_n = Vec3::new(a[0], a[1], a[2]);
    let b_n = Vec3::new(b[0], b[1], b[2]);
    if a_n.length_squared() <= 0.5 || b_n.length_squared() <= 0.5 {
        return false;
    }
    let alignment = a_n.normalize().dot(b_n.normalize());
    let plane_delta = if alignment >= 0.0 {
        (a[3] - b[3]).abs()
    } else {
        (a[3] + b[3]).abs()
    };
    alignment.abs() > 0.999 && plane_delta < 2.0
}

pub(in crate::renderer) fn select_planar_reflection_views(
    world: &WorldGpu,
    camera: &Camera,
    main_view_proj: Mat4,
    pvs_mode: PvsMode,
    area_mask: Option<&[u8; 32]>,
    planar_mode: PlanarReflectionMode,
    reflection_quality: ReflectionQuality,
    ocean_enabled: bool,
    previous_planes: &[Option<[f32; 4]>; PLANAR_REFLECTION_SLOTS],
) -> [Option<PlanarReflectionView>; PLANAR_REFLECTION_SLOTS] {
    let planar_budget = reflection_quality
        .planar_slot_budget()
        .min(PLANAR_REFLECTION_SLOTS);
    if planar_budget == 0 {
        return [None; PLANAR_REFLECTION_SLOTS];
    }
    let active_coarse = if pvs_mode == PvsMode::Off {
        &world.coarse_all_batches
    } else if let Some(cluster) = world.last_cluster.flatten() {
        world
            .coarse_visible_batches_by_cluster
            .get(cluster)
            .unwrap_or(&world.coarse_all_batches)
    } else {
        &world.coarse_all_batches
    };
    let effective_area_mask = effective_area_mask(world, pvs_mode, area_mask);

    // priority, crosshair coverage, screen score, representative reflector,
    // oriented plane, union NDC rectangle of its visible coplanar pieces
    let mut candidates: Vec<(u8, bool, f32, PlanarReflector, [f32; 4], (Vec2, Vec2))> = Vec::new();
    for reflector in &world.planar_reflectors {
        if (reflector.kind == PlanarReflectorKind::Ocean && !ocean_enabled)
            || !planar_reflector_enabled(planar_mode, reflector.kind)
            || !active_coarse.contains(&reflector.coarse_batch_index)
            || world
                .coarse_batches
                .get(reflector.coarse_batch_index)
                .is_some_and(|batch| !batch_area_visible(batch, effective_area_mask))
            || !aabb_intersects_clip_frustum(
                reflector.bounds_min,
                reflector.bounds_max,
                main_view_proj,
            )
        {
            continue;
        }
        let mut plane = reflector.plane;
        let mut normal = Vec3::new(plane[0], plane[1], plane[2]);
        let signed_distance = normal.dot(camera.position) + plane[3];
        if signed_distance.abs() < 0.25
            || matches!(reflector.cull, CullMode::Back) && signed_distance <= 0.0
            || matches!(reflector.cull, CullMode::Front) && signed_distance >= 0.0
        {
            continue;
        }
        if signed_distance < 0.0 {
            normal = -normal;
            plane = [-plane[0], -plane[1], -plane[2], -plane[3]];
        }
        let Some(ndc_rect) = projected_planar_ndc_rect(
            reflector.bounds_min,
            reflector.bounds_max,
            plane,
            main_view_proj,
        ) else {
            continue;
        };
        let (contains_centre, screen_importance) =
            planar_importance_from_ndc_rect(ndc_rect.0, ndc_rect.1);
        let minimum_screen_importance = if reflector.kind == PlanarReflectorKind::Authored {
            // Authored `portal` mirrors are baseline JKA content. If their plane
            // is actually visible, keep them eligible regardless of enhanced
            // reflection quality. The slot budget still limits total cost.
            0.0
        } else {
            match reflection_quality {
                ReflectionQuality::High => 0.015,
                ReflectionQuality::Ultra => 0.002,
                _ => 1.0,
            }
        };
        if screen_importance < minimum_screen_importance {
            continue;
        }
        let distance = (normal.dot(camera.position) + plane[3]).abs();
        // Explicit reflection owners must not lose the small planar budget to
        // opportunistic tcGen-environment promotion. In particular, a promoted
        // ocean is a real reflection consumer; if a generic environment plane
        // steals the only slot, ocean_environment() abruptly falls back to the
        // map sky as the camera moves. Keep JKA-authored mirrors highest, oceans
        // next, and generic environment candidates last.
        let priority = match reflector.kind {
            PlanarReflectorKind::Authored => 2,
            PlanarReflectorKind::Ocean => 1,
            PlanarReflectorKind::Environment => 0,
        };
        let roughness_weight = world
            .coarse_batches
            .get(reflector.coarse_batch_index)
            .map_or(1.0, |batch| {
                (1.15 - batch.source.reflection_roughness_hint).clamp(0.15, 1.15)
            });
        // Screen importance ranks reflectors *within* the semantic priority
        // tier. This keeps selection responsive between multiple oceans/mirrors
        // without allowing generic tcGen-environment candidates to make an
        // explicit ocean's reflection source flicker between planar and sky.
        let authored_weight = if reflector.kind == PlanarReflectorKind::Authored {
            1.75
        } else {
            1.0
        };
        let centre_weight = if contains_centre { 1.20 } else { 1.0 };
        let tie_break = reflector.area / (distance * distance + 4096.0);
        let score = screen_importance * roughness_weight * authored_weight * centre_weight
            + tie_break * 1e-6;
        let candidate = (
            priority,
            contains_centre,
            score,
            *reflector,
            plane,
            ndc_rect,
        );

        // Coplanar pieces share one reflected camera/texture layer. Keep the
        // strongest representative for PVS selection, but do not spend another
        // reflection slot on the same geometric plane.
        if let Some(existing) = candidates
            .iter_mut()
            .find(|existing| planar_planes_equivalent(existing.4, plane))
        {
            let candidate_is_better = score > existing.2;
            let union = (existing.5 .0.min(ndc_rect.0), existing.5 .1.max(ndc_rect.1));
            if candidate_is_better {
                *existing = candidate;
            }
            existing.5 = union;
        } else {
            candidates.push(candidate);
        }
    }

    candidates.sort_by(|a, b| {
        let a_sticky = previous_planes
            .iter()
            .flatten()
            .any(|plane| planar_planes_equivalent(*plane, a.4));
        let b_sticky = previous_planes
            .iter()
            .flatten()
            .any(|plane| planar_planes_equivalent(*plane, b.4));
        let a_score = a.2
            * if a_sticky {
                PLANAR_REFLECTION_HYSTERESIS
            } else {
                1.0
            };
        let b_score = b.2
            * if b_sticky {
                PLANAR_REFLECTION_HYSTERESIS
            } else {
                1.0
            };
        b.0.cmp(&a.0)
            .then_with(|| b_score.total_cmp(&a_score))
            .then_with(|| b.1.cmp(&a.1))
    });
    candidates.truncate(planar_budget);

    // Preserve the relative order of surviving planes so a plane normally keeps
    // the same texture-array layer. Compact to the front when one disappears,
    // then append newly-important planes. This avoids A/B slot thrash while
    // still allowing genuinely better reflectors to enter the four-view budget.
    let mut ordered = Vec::with_capacity(candidates.len());
    let mut used = vec![false; candidates.len()];
    for previous in previous_planes.iter().flatten() {
        if let Some((index, candidate)) =
            candidates.iter().enumerate().find(|(index, candidate)| {
                !used[*index] && planar_planes_equivalent(*previous, candidate.4)
            })
        {
            used[index] = true;
            ordered.push(*candidate);
        }
    }
    for (index, candidate) in candidates.iter().enumerate() {
        if !used[index] {
            ordered.push(*candidate);
        }
    }

    let mut views = [None; PLANAR_REFLECTION_SLOTS];
    for (slot, (_, _, _, reflector, plane, ndc_rect)) in ordered.into_iter().enumerate() {
        let plane_normal = Vec3::new(plane[0], plane[1], plane[2]).normalize_or_zero();
        let reflected_position = reflect_point_across_plane(camera.position, plane);
        let reflected_forward = (camera.forward()
            - 2.0 * plane_normal * camera.forward().dot(plane_normal))
        .normalize_or_zero();
        let reflected_view_proj = main_view_proj * reflection_matrix(plane);
        let pvs_origin = if reflector.kind == PlanarReflectorKind::Authored {
            Vec3::from_array(reflector.pvs_origin)
        } else {
            Vec3::from_array(reflector.pvs_origin) + plane_normal
        };
        let cluster = world
            .visibility
            .as_ref()
            .and_then(|vis| vis.cluster_at(scene::jka_position(pvs_origin.to_array())));
        views[slot] = Some(PlanarReflectionView {
            kind: reflector.kind,
            plane,
            view_proj: reflected_view_proj,
            camera_position: reflected_position,
            camera_forward: reflected_forward,
            cluster,
            coarse_batch_index: reflector.coarse_batch_index,
            cull_view_proj: ndc_rect_narrowing(ndc_rect.0, ndc_rect.1) * reflected_view_proj,
        });
    }
    views
}

pub(in crate::renderer) fn create_planar_reflection_resources(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    color_format: wgpu::TextureFormat,
    width: u32,
    height: u32,
    active: bool,
) -> PlanarReflectionResources {
    let width = width.max(1);
    let height = height.max(1);
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA planar reflection color array"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: PLANAR_REFLECTION_SLOTS as u32,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: color_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let color_array_view = color.create_view(&wgpu::TextureViewDescriptor {
        label: Some("JKA planar reflection color array view"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        base_array_layer: 0,
        array_layer_count: Some(PLANAR_REFLECTION_SLOTS as u32),
        ..Default::default()
    });
    let color_views = (0..PLANAR_REFLECTION_SLOTS)
        .map(|slot| {
            color.create_view(&wgpu::TextureViewDescriptor {
                label: Some("JKA planar reflection color layer"),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: slot as u32,
                array_layer_count: Some(1),
                ..Default::default()
            })
        })
        .collect();
    let fallback_color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA planar reflection fallback array"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: PLANAR_REFLECTION_SLOTS as u32,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: color_format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let fallback_view = fallback_color.create_view(&wgpu::TextureViewDescriptor {
        label: Some("JKA planar reflection fallback array view"),
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        base_array_layer: 0,
        array_layer_count: Some(PLANAR_REFLECTION_SLOTS as u32),
        ..Default::default()
    });
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA planar reflection depth"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("JKA planar reflection sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let uniform = PlanarReflectionUniform {
        planes: [[0.0; 4]; PLANAR_REFLECTION_SLOTS],
        viewport: [
            width as f32,
            height as f32,
            if active { 1.0 } else { 0.0 },
            0.0,
        ],
        debug: [0.0; 4],
    };
    let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA planar reflection uniform"),
        contents: bytemuck::bytes_of(&uniform),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let reflection_pass_uniform = PlanarReflectionUniform {
        planes: [[0.0; 4]; PLANAR_REFLECTION_SLOTS],
        viewport: [width as f32, height as f32, 0.0, 0.0],
        debug: [0.0; 4],
    };
    let reflection_pass_uniform_buffer =
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA planar reflection recursion-disabled uniform"),
            contents: bytemuck::bytes_of(&reflection_pass_uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA planar reflection bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&color_array_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniform_buffer.as_entire_binding(),
            },
        ],
    });
    let reflection_pass_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA planar reflection recursion-safe bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&fallback_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: reflection_pass_uniform_buffer.as_entire_binding(),
            },
        ],
    });
    PlanarReflectionResources {
        _color: color,
        color_views,
        _color_array_view: color_array_view,
        _fallback_color: fallback_color,
        _depth: depth,
        depth_view,
        _sampler: sampler,
        uniform_buffer,
        _reflection_pass_uniform_buffer: reflection_pass_uniform_buffer,
        bind_group,
        reflection_pass_bind_group,
        _width: width,
        _height: height,
        active,
    }
}

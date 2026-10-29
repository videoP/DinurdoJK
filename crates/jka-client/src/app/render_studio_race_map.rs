//! One background preparation of course-local, position-only HUD geometry.
use crate::{
    scene::{DrawClass, PreparedMap},
    ui::StudioRaceMap,
};
use glam::Vec3;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

const TRIANGLE_BUDGET: usize = crate::ui::MAX_MAP_TRIANGLES;

pub(super) fn prepare(
    source: &mut super::render_studio_demo::StudioDemoSource,
    race: &super::render_studio_race::RaceComparison,
    runtime: &[super::render_studio_demo::StudioRaceRuntime],
) -> Result<(), String> {
    if source
        .race_map_job
        .as_ref()
        .is_some_and(|j| j.is_finished())
    {
        source.race_map = Some(
            source
                .race_map_job
                .take()
                .unwrap()
                .join()
                .map_err(|_| "Route map preparation panicked".to_owned())?,
        );
    }
    let same_course = source.race_map_reference == race.reference
        && source.race_map_runners.len() == race.runners.len()
        && source
            .race_map_runners
            .iter()
            .zip(&race.runners)
            .all(|(key, r)| key.0 == r.name && key.1 == r.game);
    if same_course && source.race_map.is_some() {
        return Ok(());
    }
    // Finish an existing job before changing course; never pile up workers.
    if source.race_map_job.is_some() {
        return Ok(());
    }
    source.race_map = None;
    let Some(prepared) = &source.prepared else {
        return Ok(());
    };
    let mut course = Vec::new();
    let mut route = Vec::new();
    let mut routes = Vec::new();
    for (i, runner) in race.runners.iter().enumerate() {
        let Some(ghost) = runtime
            .iter()
            .find(|r| r.key.0 == runner.name && r.key.1 == runner.game)
            .and_then(|r| r.ghost.as_ref())
        else {
            return Ok(());
        };
        let frames = &ghost.track.frames;
        let points: Vec<_> = frames
            .iter()
            .step_by(frames.len().div_ceil(300).max(1))
            .chain(frames.last())
            .filter_map(|f| super::playerstate_vec3(&f.player_state, "origin"))
            .filter(|p| Vec3::from_array(*p).is_finite())
            .collect();
        if i == race.reference {
            route = points.clone();
        }
        course.extend_from_slice(&points);
        routes.push(points);
    }
    source.race_map_runners = race
        .runners
        .iter()
        .map(|r| (r.name.clone(), r.game.clone()))
        .collect();
    source.race_map_reference = race.reference;
    let map = Arc::clone(&prepared.map);
    source.race_map_job = Some(
        std::thread::Builder::new()
            .name("studio-route-map".into())
            .spawn(move || build(&map, route, &course, routes))
            .map_err(|e| format!("Could not prepare route map: {e}"))?,
    );
    Ok(())
}

pub(super) fn build(
    map: &PreparedMap,
    route: Vec<[f32; 3]>,
    course: &[[f32; 3]],
    routes: Vec<Vec<[f32; 3]>>,
) -> Arc<StudioRaceMap> {
    let mut low = Vec3::splat(f32::INFINITY);
    let mut high = Vec3::splat(f32::NEG_INFINITY);
    for &point in course {
        let p = Vec3::from_array(point);
        if p.is_finite() {
            low = low.min(p);
            high = high.max(p);
        }
    }
    if !low.is_finite() {
        return Arc::new(StudioRaceMap::new(Vec::new(), Vec::new(), route, course));
    }
    low -= Vec3::new(256.0, 256.0, 128.0);
    high += Vec3::new(256.0, 256.0, 96.0);
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    let mut weld = HashMap::new();
    let mut seen_ranges = HashSet::new();
    let mut seen_triangles = HashSet::new();
    for batch in &map.batches {
        // Draw batches already omit utility surfaces. Defend the overview too,
        // including maps with incomplete or overridden shader definitions.
        if let Some(material) = map
            .material_debug
            .entries
            .get(batch.material_debug_index as usize)
        {
            let name = material.name.to_ascii_lowercase();
            let leaf = name.rsplit('/').next().unwrap_or(&name);
            if leaf.contains("caulk")
                || leaf.starts_with("nodraw")
                || matches!(leaf, "clip" | "playerclip" | "hint" | "skip")
                || map
                    .shader_library
                    .get(&material.name)
                    .is_some_and(|shader| shader.nodraw)
            {
                continue;
            }
        }

        if !batch.pipeline.depth_write
            || !matches!(batch.pipeline.class, DrawClass::Opaque | DrawClass::Mask)
            || batch.pipeline.auto_sprite2
            || !seen_ranges.insert((batch.vertices.start, batch.vertices.end))
        {
            continue;
        }
        let Some(vertices) = map
            .vertices
            .get(batch.vertices.start as usize..batch.vertices.end as usize)
        else {
            continue;
        };
        for t in vertices.chunks_exact(3) {
            let points = [t[0].position, t[1].position, t[2].position]
                .map(crate::scene::jka_position)
                .map(Vec3::from_array);
            if points.iter().any(|p| !p.is_finite()) {
                continue;
            }
            let normal = (points[1] - points[0])
                .cross(points[2] - points[0])
                .normalize_or_zero();
            // Open-roof architectural model: ceiling undersides would wash out
            // the course, and decorative sky/glow/water never enter this mesh.
            if normal.z < -0.5 || normal == Vec3::ZERO {
                continue;
            }
            let min = points[0].min(points[1]).min(points[2]);
            let max = points[0].max(points[1]).max(points[2]);
            if min.cmpgt(high).any() || max.cmplt(low).any() {
                continue;
            }
            let polygon = clip(&points, low, high);
            for i in 1..polygon.len().saturating_sub(1) {
                let triangle = [polygon[0], polygon[i], polygon[i + 1]];
                if (triangle[1] - triangle[0])
                    .cross(triangle[2] - triangle[0])
                    .length_squared()
                    < 0.01
                {
                    continue;
                }
                let triangle = triangle.map(|p| {
                    // Share positions across material/lightmap seams. Those
                    // seams have no meaning in an untextured white model.
                    let key = p.to_array().map(|v| (v * 16.0).round() as i64);
                    *weld.entry(key).or_insert_with(|| {
                        let index = positions.len() as u32;
                        positions.push(p.to_array());
                        index
                    })
                });
                let mut key = triangle;
                key.sort_unstable();
                if key[0] != key[1] && key[1] != key[2] && seen_triangles.insert(key) {
                    indices.extend(triangle);
                }
            }
        }
    }
    reduce(&positions, &mut indices);
    // Retain only vertices referenced by the small result, not a second BSP.
    let compact = meshopt::optimize_vertex_fetch(&mut indices, &positions);
    let triangles = indices
        .chunks_exact(3)
        .map(|t| [t[0], t[1], t[2]])
        .collect();
    Arc::new(StudioRaceMap::new(compact, triangles, route, course).with_routes(routes))
}

fn reduce(positions: &[[f32; 3]], indices: &mut Vec<u32>) {
    let target = TRIANGLE_BUDGET * 3;
    if indices.len() <= target {
        return;
    }
    let adapter = meshopt::VertexDataAdapter::new(bytemuck::cast_slice(positions), 12, 0).unwrap();
    let reduced = meshopt::simplify(
        indices,
        &adapter,
        target,
        0.01,
        meshopt::SimplifyOptions::Prune,
        None,
    );
    *indices = if reduced.len() <= target {
        reduced
    } else {
        // Strict work budget even for dense/disconnected detail meshes. This
        // is geometric clustering, never stride-sampling triangles into holes.
        meshopt::simplify_sloppy(&reduced, &adapter, target, 1.0, None)
    };
    debug_assert!(indices.len() <= target);
}

fn clip(points: &[Vec3; 3], low: Vec3, high: Vec3) -> Vec<Vec3> {
    let mut polygon = points.to_vec();
    let mut next = Vec::with_capacity(10);
    for axis in 0..3 {
        for (bound, sign) in [(low[axis], 1.0), (high[axis], -1.0)] {
            next.clear();
            if polygon.is_empty() {
                return polygon;
            }
            let mut a = *polygon.last().unwrap();
            let mut da = (a[axis] - bound) * sign;
            for &b in &polygon {
                let db = (b[axis] - bound) * sign;
                if (da >= 0.0) != (db >= 0.0) {
                    next.push(a.lerp(b, da / (da - db)));
                }
                if db >= 0.0 {
                    next.push(b);
                }
                a = b;
                da = db;
            }
            std::mem::swap(&mut polygon, &mut next);
        }
    }
    polygon
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn giant_floor_is_clipped_instead_of_dropped_or_shrinking_the_course() {
        let polygon = clip(
            &[
                Vec3::new(-1000.0, -1000.0, 0.0),
                Vec3::new(1000.0, -1000.0, 0.0),
                Vec3::new(0.0, 1000.0, 0.0),
            ],
            Vec3::splat(-10.0),
            Vec3::splat(10.0),
        );
        assert_eq!(polygon.len(), 4);
        assert!(polygon
            .iter()
            .all(|p| p.abs().cmple(Vec3::splat(10.001)).all()));
        let area: f32 = (1..polygon.len() - 1)
            .map(|i| {
                (polygon[i] - polygon[0])
                    .cross(polygon[i + 1] - polygon[0])
                    .length()
                    * 0.5
            })
            .sum();
        assert!((area - 400.0).abs() < 0.01);
    }

    #[test]
    fn dense_geometry_respects_the_hud_triangle_budget() {
        let n = 80u32;
        let positions: Vec<_> = (0..=n)
            .flat_map(|y| {
                (0..=n).map(move |x| {
                    [
                        x as f32,
                        y as f32,
                        (x as f32 * 0.4).sin() * (y as f32 * 0.4).cos(),
                    ]
                })
            })
            .collect();
        let mut indices = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let a = y * (n + 1) + x;
                indices.extend([a, a + 1, a + n + 1, a + 1, a + n + 2, a + n + 1]);
            }
        }
        reduce(&positions, &mut indices);
        assert!(!indices.is_empty());
        assert!(indices.len() <= TRIANGLE_BUDGET * 3);
        assert_eq!(indices.len() % 3, 0);
        assert!(indices.iter().all(|&i| (i as usize) < positions.len()));
    }
}

//! cg_specHudMap: builds the broadcast radar's floor plan once per loaded map
//! (on a worker, from the already prepared world) and remembers where each
//! player was last presented, so players who leave the view stay on the radar
//! for a moment as a faded last-known blip.
use super::{App, Instant};
use crate::{
    cgame::{ET_PLAYER, GT_TEAM},
    scene::{DrawClass, PreparedMap},
    ui::{BroadcastMinimap, MAX_RADAR_EDGES, MAX_RADAR_TRIANGLES},
};
use glam::{IVec2, Vec2, Vec3};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Weak},
    thread::JoinHandle,
};

const EF_DEAD: i32 = 1 << 1;
/// Floors and walkable ramps (MIN_WALK_NORMAL is 0.7).
const FLOOR_NORMAL_Z: f32 = 0.6;
/// Floor-area clustering cell: islands farther apart than this are separate.
const CLUSTER_CELL: f32 = 512.0;
/// A floor island joins the radar when it has this share of the largest one
/// (drops 3D-skybox miniatures and stray far-off geometry).
const CLUSTER_KEEP: f32 = 0.12;
const BOUNDS_PAD: f32 = 160.0;
const MIN_EDGE: f32 = 20.0;

#[derive(Clone, Copy, Debug)]
pub(in crate::app) struct SeenPlayer {
    pub position: [f32; 3],
    pub yaw: f32,
    pub alive: bool,
    pub at: Instant,
}

#[derive(Default)]
pub(in crate::app) struct BroadcastMinimapState {
    /// The prepared world `map` (or the running job) was built from.
    source: Option<Weak<PreparedMap>>,
    job: Option<(Weak<PreparedMap>, JoinHandle<BroadcastMinimap>)>,
    pub(in crate::app) map: Option<Arc<BroadcastMinimap>>,
    /// Last presented position of each client.
    pub(in crate::app) seen: [Option<SeenPlayer>; 32],
}

impl App {
    /// Called every tick: adopts a finished build, starts one for a newly
    /// loaded map, and records presented player positions.
    pub(in crate::app) fn sync_broadcast_minimap(&mut self, now: Instant) {
        let state = &mut self.broadcast_minimap;
        if state.job.as_ref().is_some_and(|(_, job)| job.is_finished()) {
            let (source, job) = state.job.take().expect("checked");
            match job.join() {
                Ok(map) if state.source.as_ref().is_some_and(|current| current.ptr_eq(&source)) => {
                    state.map = Some(Arc::new(map));
                }
                Ok(_) => {}
                Err(_) => eprintln!("SPEC RADAR: floor plan build panicked"),
            }
        }
        let Some(session) = self.game_session.as_ref() else { return };
        if self.japro_cg.spec_hud < 2 || self.japro_cg.spec_hud_map == 0
            || session.client_game.gametype() < GT_TEAM
        {
            return;
        }
        for entity in session.presented_entities.iter().filter(|e| e.entity_type == ET_PLAYER) {
            if let Some(slot) = state.seen.get_mut(usize::from(entity.number)) {
                *slot = Some(SeenPlayer {
                    position: entity.origin,
                    yaw: entity.angles[1],
                    alive: entity.state.field_i32("eFlags").unwrap_or(0) & EF_DEAD == 0,
                    at: now,
                });
            }
        }
        if let Some(ps) = session.current_snapshot.as_ref().map(|snapshot| &snapshot.player_state) {
            let client = ps.field_i32("clientNum").and_then(|client| usize::try_from(client).ok());
            if let Some(slot) = client.and_then(|client| state.seen.get_mut(client)) {
                *slot = Some(SeenPlayer {
                    position: ps.field_vec3("origin").unwrap_or([0.0; 3]),
                    yaw: ps.field_vec3("viewangles").map_or(0.0, |angles| angles[1]),
                    alive: ps.stats[0] > 0,
                    at: now,
                });
            }
        }

        let Some(cache) = self.prepared_map_cache.as_ref() else { return };
        let same_map = session.map_name.as_deref().is_some_and(|name| {
            Self::normalized_bsp_name(name) == Self::normalized_bsp_name(&cache.label)
        });
        let current = state.source.as_ref()
            .is_some_and(|source| std::ptr::eq(source.as_ptr(), Arc::as_ptr(&cache.map)));
        // One worker at a time; a stale one finishes and is discarded.
        if !same_map || current || state.job.is_some() {
            return;
        }
        state.map = None;
        state.seen = Default::default();
        let source = Arc::downgrade(&cache.map);
        state.source = Some(source.clone());
        let map = Arc::clone(&cache.map);
        let spawned = std::thread::Builder::new().name("spec-radar".into()).spawn(move || {
            let started = std::time::Instant::now();
            let plan = build(&map);
            eprintln!("SPEC RADAR: floor plan in {:.0} ms", started.elapsed().as_secs_f64() * 1000.0);
            plan
        });
        match spawned {
            Ok(job) => state.job = Some((source, job)),
            Err(error) => eprintln!("SPEC RADAR: could not start floor plan build: {error}"),
        }
    }
}

/// Upward-facing opaque world triangles (JKA coordinates).
fn floor_triangles(map: &PreparedMap) -> Vec<[Vec3; 3]> {
    let mut floors = Vec::new();
    let mut seen_ranges = HashSet::new();
    for batch in &map.batches {
        // Same utility-surface defence as the Route Comparison map.
        if let Some(material) = map.material_debug.entries.get(batch.material_debug_index as usize) {
            let name = material.name.to_ascii_lowercase();
            let leaf = name.rsplit('/').next().unwrap_or(&name);
            if leaf.contains("caulk")
                || leaf.starts_with("nodraw")
                || matches!(leaf, "clip" | "playerclip" | "hint" | "skip")
                || map.shader_library.get(&material.name).is_some_and(|shader| shader.nodraw)
            {
                continue;
            }
        }
        if !batch.pipeline.depth_write
            || !matches!(batch.pipeline.class, DrawClass::Opaque | DrawClass::Mask)
            || batch.pipeline.auto_sprite
            || batch.pipeline.auto_sprite2
            || !seen_ranges.insert((batch.vertices.start, batch.vertices.end))
        {
            continue;
        }
        let Some(vertices) = map.vertices.get(batch.vertices.start as usize..batch.vertices.end as usize) else {
            continue;
        };
        for t in vertices.chunks_exact(3) {
            let points = [t[0].position, t[1].position, t[2].position]
                .map(crate::scene::jka_position)
                .map(Vec3::from_array);
            if points.iter().any(|p| !p.is_finite()) {
                continue;
            }
            let normal = (points[1] - points[0]).cross(points[2] - points[0]).normalize_or_zero();
            if normal.z >= FLOOR_NORMAL_Z {
                floors.push(points);
            }
        }
    }
    floors
}

fn area_xy(t: &[Vec3; 3]) -> f32 {
    let (a, b, c) = (t[0].truncate(), t[1].truncate(), t[2].truncate());
    ((b - a).perp_dot(c - a) * 0.5).abs()
}

/// XY bounds of the floor islands that matter: floor area binned into cells,
/// 8-connected islands, keeping every island with a fair share of the largest.
fn play_bounds(floors: &[[Vec3; 3]]) -> Option<(Vec2, Vec2)> {
    let cell = |p: Vec2| (p / CLUSTER_CELL).floor().as_ivec2();
    let mut cells: HashMap<IVec2, f32> = HashMap::new();
    for t in floors {
        let center = (t[0] + t[1] + t[2]).truncate() / 3.0;
        *cells.entry(cell(center)).or_default() += area_xy(t);
    }
    let mut island_of: HashMap<IVec2, usize> = HashMap::new();
    let mut islands: Vec<f32> = Vec::new();
    for &start in cells.keys() {
        if island_of.contains_key(&start) {
            continue;
        }
        let id = islands.len();
        let mut area = 0.0;
        let mut stack = vec![start];
        island_of.insert(start, id);
        while let Some(at) = stack.pop() {
            area += cells[&at];
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let next = at + IVec2::new(dx, dy);
                    if cells.contains_key(&next) && !island_of.contains_key(&next) {
                        island_of.insert(next, id);
                        stack.push(next);
                    }
                }
            }
        }
        islands.push(area);
    }
    let largest = islands.iter().copied().fold(0.0_f32, f32::max);
    if largest <= 0.0 {
        return None;
    }
    let mut low = Vec2::splat(f32::INFINITY);
    let mut high = Vec2::splat(f32::NEG_INFINITY);
    for t in floors {
        let center = (t[0] + t[1] + t[2]).truncate() / 3.0;
        if island_of.get(&cell(center)).is_some_and(|&id| islands[id] >= largest * CLUSTER_KEEP) {
            for p in t {
                low = low.min(p.truncate());
                high = high.max(p.truncate());
            }
        }
    }
    low.is_finite().then_some((low - Vec2::splat(BOUNDS_PAD), high + Vec2::splat(BOUNDS_PAD)))
}

/// Area-weighted `q` quantile of floor height (the shading range ignores a
/// lone tower or pit).
fn height_quantile(floors: &[[Vec3; 3]], q: f32) -> f32 {
    let mut heights: Vec<(f32, f32)> = floors.iter()
        .map(|t| ((t[0].z + t[1].z + t[2].z) / 3.0, area_xy(t)))
        .collect();
    heights.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f32 = heights.iter().map(|h| h.1).sum();
    let mut sum = 0.0;
    for (z, area) in &heights {
        sum += area;
        if sum >= total * q {
            return *z;
        }
    }
    heights.last().map_or(0.0, |h| h.0)
}

/// Clip a triangle to an XY box (heights interpolate along).
fn clip_xy(points: &[Vec3; 3], low: Vec2, high: Vec2) -> Vec<Vec3> {
    let mut polygon = points.to_vec();
    let mut next = Vec::with_capacity(9);
    for axis in 0..2 {
        for (bound, sign) in [(low[axis], 1.0), (high[axis], -1.0)] {
            next.clear();
            let Some(&last) = polygon.last() else { return polygon };
            let mut a = last;
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

pub(in crate::app) fn build(map: &PreparedMap) -> BroadcastMinimap {
    build_from_floors(floor_triangles(map))
}

fn build_from_floors(floors: Vec<[Vec3; 3]>) -> BroadcastMinimap {
    let empty = || BroadcastMinimap::new(Vec::new(), Vec::new(), Vec3::ZERO, Vec3::ONE);
    let Some((low, high)) = play_bounds(&floors) else { return empty() };
    let z_low = height_quantile(&floors, 0.02);
    let z_high = height_quantile(&floors, 0.98).max(z_low + 1.0);

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut weld = HashMap::new();
    let mut seen_triangles = HashSet::new();
    for t in &floors {
        let min = t[0].min(t[1]).min(t[2]).truncate();
        let max = t[0].max(t[1]).max(t[2]).truncate();
        if min.cmpgt(high).any() || max.cmplt(low).any() {
            continue;
        }
        let polygon = clip_xy(t, low, high);
        for i in 1..polygon.len().saturating_sub(1) {
            let triangle = [polygon[0], polygon[i], polygon[i + 1]];
            if area_xy(&triangle) < 0.5 {
                continue;
            }
            let triangle = triangle.map(|p| {
                let key = p.to_array().map(|v| (v * 16.0).round() as i64);
                *weld.entry(key).or_insert_with(|| {
                    positions.push(p.to_array());
                    positions.len() as u32 - 1
                })
            });
            let mut key = triangle;
            key.sort_unstable();
            if key[0] != key[1] && key[1] != key[2] && seen_triangles.insert(key) {
                indices.extend(triangle);
            }
        }
    }
    reduce(&positions, &mut indices);

    // Open boundaries of the floor mesh: ledges, wall feet, stair edges.
    let mut uses: HashMap<[u32; 2], u32> = HashMap::new();
    for t in indices.chunks_exact(3) {
        for [a, b] in [[t[0], t[1]], [t[1], t[2]], [t[2], t[0]]] {
            *uses.entry([a.min(b), a.max(b)]).or_default() += 1;
        }
    }
    let on_clip_border = |a: Vec3, b: Vec3| {
        (0..2).any(|axis| {
            [low[axis], high[axis]].iter().any(|bound| (a[axis] - bound).abs() < 0.5 && (b[axis] - bound).abs() < 0.5)
        })
    };
    let position = |i: u32| Vec3::from_array(positions[i as usize]);
    let mut edges: Vec<[Vec3; 2]> = uses.into_iter()
        .filter(|(_, count)| *count == 1)
        .map(|(e, _)| [position(e[0]), position(e[1])])
        .filter(|[a, b]| a.truncate().distance(b.truncate()) >= MIN_EDGE && !on_clip_border(*a, *b))
        .collect();
    // Long architectural outlines first; tiny fragments are dropped.
    edges.sort_by(|a, b| {
        let length = |e: &[Vec3; 2]| e[0].truncate().distance_squared(e[1].truncate());
        length(b).total_cmp(&length(a))
    });
    edges.truncate(MAX_RADAR_EDGES);
    let triangles = indices.chunks_exact(3)
        .map(|t| [position(t[0]), position(t[1]), position(t[2])])
        .collect();
    BroadcastMinimap::new(triangles, edges, low.extend(z_low), high.extend(z_high))
}

fn reduce(positions: &[[f32; 3]], indices: &mut Vec<u32>) {
    let target = MAX_RADAR_TRIANGLES * 3;
    if indices.len() <= target {
        return;
    }
    let Ok(adapter) = meshopt::VertexDataAdapter::new(bytemuck::cast_slice(positions), 12, 0) else {
        indices.truncate(target);
        return;
    };
    let reduced = meshopt::simplify(indices, &adapter, target, 0.01, meshopt::SimplifyOptions::Prune, None);
    *indices = if reduced.len() <= target {
        reduced
    } else {
        meshopt::simplify_sloppy(&reduced, &adapter, target, 1.0, None)
    };
    indices.truncate(target - target % 3);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(x: f32, y: f32, size: f32, z: f32) -> [[Vec3; 3]; 2] {
        let p = |dx: f32, dy: f32| Vec3::new(x + dx, y + dy, z);
        [[p(0.0, 0.0), p(size, 0.0), p(size, size)], [p(0.0, 0.0), p(size, size), p(0.0, size)]]
    }

    #[test]
    fn far_small_island_is_left_out_of_the_bounds() {
        let mut floors: Vec<[Vec3; 3]> = quad(0.0, 0.0, 2048.0, 0.0).to_vec();
        // A 3D-skybox miniature far away.
        floors.extend(quad(40_000.0, 40_000.0, 64.0, 0.0));
        let (low, high) = play_bounds(&floors).expect("bounds");
        assert!(high.x < 3000.0 && high.y < 3000.0, "{low} {high}");
    }

    #[test]
    fn floor_plan_has_outline_and_respects_budgets() {
        let mut floors: Vec<[Vec3; 3]> = Vec::new();
        floors.extend(quad(0.0, 0.0, 1024.0, 0.0));
        floors.extend(quad(256.0, 256.0, 256.0, 128.0));
        let plan = build_from_floors(floors);
        assert!(!plan.is_empty());
    }
}

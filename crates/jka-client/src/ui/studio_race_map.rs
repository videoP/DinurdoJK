//! Nearby ledge outlines in the existing HUD batch: no world render or GPU resources.
use super::{StudioRaceUi, UiVertex};
use glam::{Vec2, Vec3};
use std::{collections::BTreeMap, sync::Mutex};

pub(crate) const MAX_MAP_TRIANGLES: usize = 1536;
const MAX_MAP_EDGES: usize = 384;
const ISO_SIN: f32 = 0.25881905; // Local cutaway, 15 degrees above horizontal.
const ISO_COS: f32 = 0.9659258;

#[derive(Debug)]
pub(crate) struct StudioRaceMap {
    vertices: Vec<Vec3>,
    triangles: Vec<[u32; 3]>,
    edges: Vec<[u32; 2]>,
    route: Vec<[f32; 3]>,
    center: Vec3,
    routes: Vec<Vec<[f32; 3]>>,
    // A single bounded entry shared by cloned UI snapshots. The slow orbit only
    // needs 15 Hz reprojection/sorting; players are still sampled every frame.
    cache: Mutex<MapCache>,
}

#[derive(Debug, Default)]
struct MapCache {
    key: Option<(u32, u32, i64, [i32; 3])>,
    vertices: Vec<UiVertex>,
}

impl StudioRaceMap {
    pub(crate) fn new(
        vertices: Vec<[f32; 3]>,
        triangles: Vec<[u32; 3]>,
        route: Vec<[f32; 3]>,
        course: &[[f32; 3]],
    ) -> Self {
        debug_assert!(triangles.len() <= MAX_MAP_TRIANGLES);
        let vertices: Vec<_> = vertices.into_iter().map(Vec3::from_array).collect();
        let mut edges = BTreeMap::new();
        for t in &triangles {
            let [a, b, c] = t.map(|i| vertices[i as usize]);
            let normal = (b - a).cross(c - a).normalize_or_zero();
            for [a, b] in [[t[0], t[1]], [t[1], t[2]], [t[2], t[0]]] {
                let key = [a.min(b), a.max(b)];
                let entry = edges.entry(key).or_insert((normal, 0usize, false, false));
                entry.1 += 1;
                entry.2 |= entry.0.dot(normal).abs() < 0.96;
                entry.3 |= normal.z > 0.65;
            }
        }
        // Keep boundaries and architectural creases; omit coplanar diagonals.
        let mut edges: Vec<_> = edges
            .into_iter()
            .filter_map(|(e, (_, n, crease, ledge))| {
                if !ledge || !(n == 1 || crease) {
                    return None;
                }
                let a = vertices[e[0] as usize];
                let b = vertices[e[1] as usize];
                [a, b, (a + b) * 0.5]
                    .iter()
                    .any(|p| {
                        course
                            .iter()
                            .any(|q| p.distance_squared(Vec3::from_array(*q)) <= 220.0 * 220.0)
                    })
                    .then_some(e)
            })
            .collect();
        // Prefer long architectural outlines over tiny decorative fragments.
        edges.sort_unstable_by(|a, b| {
            let length =
                |e: &[u32; 2]| (vertices[e[0] as usize] - vertices[e[1] as usize]).length_squared();
            length(b).total_cmp(&length(a)).then(a.cmp(b))
        });
        edges.truncate(MAX_MAP_EDGES);
        let mut low = Vec3::splat(f32::INFINITY);
        let mut high = Vec3::splat(f32::NEG_INFINITY);
        for p in vertices
            .iter()
            .copied()
            .chain(course.iter().copied().map(Vec3::from_array))
        {
            if p.is_finite() {
                low = low.min(p);
                high = high.max(p);
            }
        }
        if !low.is_finite() {
            low = Vec3::ZERO;
            high = Vec3::ONE;
        }
        let center = (low + high) * 0.5;
        Self {
            vertices,
            triangles,
            edges,
            route,
            center,
            routes: Vec::new(),
            cache: Mutex::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn diagnostic_mesh(&self) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        (
            self.vertices.iter().map(|v| v.to_array()).collect(),
            self.triangles.clone(),
        )
    }

    pub(crate) fn with_routes(mut self, routes: Vec<Vec<[f32; 3]>>) -> Self {
        self.routes = routes;
        self
    }

    fn geometry(&self, out: &mut Vec<UiVertex>, p: Projector, width: u32, height: u32) {
        for &[a, b] in &self.edges {
            let center = (self.vertices[a as usize] + self.vertices[b as usize]) * 0.5;
            if center.truncate().distance(p.center.truncate()) > 2400.0 {
                continue;
            }
            let a = p.project(self.vertices[a as usize]).truncate();
            let b = p.project(self.vertices[b as usize]).truncate();
            if let Some((a, b)) = p.clip_line(a, b) {
                line(
                    out,
                    a,
                    b,
                    0.7 * p.ui_scale,
                    [0.65, 0.70, 0.74, 0.5],
                    width,
                    height,
                );
            }
        }
    }

    fn paths(
        &self,
        out: &mut Vec<UiVertex>,
        race: &StudioRaceUi,
        p: Projector,
        width: u32,
        height: u32,
    ) {
        let mut total = 0;
        for primary in [false, true] {
            for runner in race
                .runners
                .iter()
                .filter(|r| (r.focused || r.reference) == primary)
            {
                let Some(path) = self.routes.get(runner.route_index) else {
                    continue;
                };
                let mut runs: Vec<Vec<Vec2>> = Vec::new();
                for pair in path.windows(2) {
                    let a3 = Vec3::from_array(pair[0]);
                    let b3 = Vec3::from_array(pair[1]);
                    if ((a3 + b3) * 0.5).truncate().distance(p.center.truncate()) > 2400.0 {
                        continue;
                    }
                    let a = p.project(Vec3::from_array(pair[0])).truncate();
                    let b = p.project(Vec3::from_array(pair[1])).truncate();
                    if let Some((a, b)) = p.clip_line(a, b) {
                        if let Some(run) = runs.last_mut().filter(|run| {
                            run.last()
                                .is_some_and(|last| last.distance_squared(a) < 0.01)
                        }) {
                            run.push(b);
                        } else {
                            runs.push(vec![a, b]);
                        }
                    }
                }
                let segments = runs.iter().map(|r| r.len() - 1).sum::<usize>();
                let budget = if primary {
                    96
                } else {
                    (320 / race.runners.len().max(1)).clamp(8, 48)
                };
                let stride = segments.div_ceil(budget).max(1);
                let color = [
                    runner.color[0] as f32 / 255.0,
                    runner.color[1] as f32 / 255.0,
                    runner.color[2] as f32 / 255.0,
                    if primary { 0.95 } else { 0.30 },
                ];
                let mut drawn = 0;
                for run in runs {
                    let points: Vec<_> = run
                        .iter()
                        .step_by(stride)
                        .chain(run.last())
                        .copied()
                        .collect();
                    for pair in points.windows(2) {
                        if drawn >= budget || total >= 512 {
                            break;
                        }
                        line(
                            out,
                            pair[0],
                            pair[1],
                            if primary {
                                1.6 * p.ui_scale
                            } else {
                                0.8 * p.ui_scale
                            },
                            color,
                            width,
                            height,
                        );
                        drawn += 1;
                        total += 1;
                    }
                }
            }
        }
        if self.routes.is_empty() {
            route(out, &self.route, p, width, height);
        }
    }
}

#[derive(Clone, Copy)]
struct Projector {
    center: Vec3,
    screen: Vec2,
    scale: f32,
    sin: f32,
    cos: f32,
    ui_scale: f32,
}

impl Projector {
    fn new(center: Vec3, extent: Vec2, tick: i64, width: u32, s: f32) -> Self {
        let panel_x = (width as f32 - 364.0 * s).max(0.0);
        let yaw = std::f64::consts::FRAC_PI_4 + (tick as f64 / 15.0) * 14.0_f64.to_radians();
        let (sin, cos) = yaw.sin_cos();
        Self {
            center,
            screen: Vec2::new(panel_x + 175.0 * s, 170.0 * s),
            scale: (157.0 * s / extent.x).min(140.0 * s / extent.y),
            sin: sin as f32,
            cos: cos as f32,
            ui_scale: s,
        }
    }

    fn bounds(self) -> (Vec2, Vec2) {
        let half = Vec2::new(157.0, 140.0) * self.ui_scale;
        (self.screen - half, self.screen + half)
    }
    fn clip_line(self, a: Vec2, b: Vec2) -> Option<(Vec2, Vec2)> {
        if !a.is_finite() || !b.is_finite() {
            return None;
        }
        let (low, high) = self.bounds();
        let d = b - a;
        let mut begin = 0.0_f32;
        let mut end = 1.0_f32;
        for axis in 0..2 {
            if d[axis].abs() < 1e-6 {
                if a[axis] < low[axis] || a[axis] > high[axis] {
                    return None;
                }
                continue;
            }
            let t0 = (low[axis] - a[axis]) / d[axis];
            let t1 = (high[axis] - a[axis]) / d[axis];
            begin = begin.max(t0.min(t1));
            end = end.min(t0.max(t1));
            if begin > end {
                return None;
            }
        }
        Some((a + d * begin, a + d * end))
    }

    fn project(self, world: Vec3) -> Vec3 {
        let v = world - self.center;
        let x = self.cos * v.x - self.sin * v.y;
        let y = self.sin * v.x + self.cos * v.y;
        Vec3::new(
            self.screen.x + x * self.scale,
            self.screen.y + (y * ISO_SIN - v.z * ISO_COS) * self.scale,
            y * ISO_COS + v.z * ISO_SIN,
        )
    }
}

pub(super) fn draw(out: &mut Vec<UiVertex>, race: &StudioRaceUi, width: u32, height: u32) {
    let tick = (race.output_time.max(0.0) * 15.0).floor() as i64;
    let s = ui_scale(race, height);
    let p = if let Some(map) = &race.map {
        let focused = race
            .runners
            .iter()
            .find(|r| r.focused)
            .map_or(map.center, |r| Vec3::from_array(r.position));
        // Bounded quantization keeps the cached mesh and pins in exactly the
        // same coordinate system without rebuilding for sub-unit movement.
        let cell = (focused / 64.0).round().as_ivec3().to_array();
        let center = Vec3::new(cell[0] as f32, cell[1] as f32, cell[2] as f32) * 64.0;
        let p = Projector::new(center, Vec2::splat(2300.0), tick, width, s);
        let mut cache = map.cache.lock().unwrap_or_else(|e| e.into_inner());
        let key = (width, height, tick, cell);
        if cache.key != Some(key) {
            cache.vertices.clear();
            map.geometry(&mut cache.vertices, p, width, height);
            cache.key = Some(key);
        }
        out.extend_from_slice(&cache.vertices);
        drop(cache);
        map.paths(out, race, p, width, height);
        p
    } else {
        // Also works while the small background mesh job is in flight.
        let mut low = Vec3::splat(f32::INFINITY);
        let mut high = Vec3::splat(f32::NEG_INFINITY);
        for point in race
            .route
            .iter()
            .chain(race.runners.iter().map(|r| &r.position))
        {
            let v = Vec3::from_array(*point);
            if v.is_finite() {
                low = low.min(v);
                high = high.max(v);
            }
        }
        if !low.is_finite() {
            return;
        }
        let half = (high - low) * 0.5;
        let radius = half.x.hypot(half.y).max(1.0);
        let p = Projector::new(
            (high + low) * 0.5,
            Vec2::new(radius, radius * ISO_SIN + half.z * ISO_COS).max(Vec2::ONE),
            tick,
            width,
            s,
        );
        route(out, &race.route, p, width, height);
        p
    };
    // Draw all markers after the translucent model, focused runner last.
    for focused in [false, true] {
        for runner in race.runners.iter().filter(|r| r.focused == focused) {
            let raw_position = p.project(Vec3::from_array(runner.position)).truncate();
            let (low, high) = p.bounds();
            let position =
                raw_position.clamp(low + Vec2::splat(12.0 * s), high - Vec2::splat(16.0 * s));
            if !position.is_finite() {
                continue;
            }
            let color = [
                runner.color[0] as f32 / 255.0,
                runner.color[1] as f32 / 255.0,
                runner.color[2] as f32 / 255.0,
                1.0,
            ];
            let tip = position - Vec2::Y * 7.0 * s;
            line(out, position, tip, 1.2 * s, color, width, height);
            let radius = if focused { 4.0 } else { 2.8 } * s;
            super::disc(
                out,
                tip.x,
                tip.y,
                radius + 1.4 * s,
                12,
                [0.01, 0.02, 0.03, 0.85],
                width,
                height,
            );
            super::disc(out, tip.x, tip.y, radius, 12, color, width, height);
            super::text::text(
                out,
                &format!("{}", runner.rank),
                tip.x + radius + 2.0 * s,
                tip.y - 3.0 * s,
                0.8 * s,
                color,
                width,
                height,
            );
        }
    }
}

pub(super) fn ui_scale(race: &StudioRaceUi, height: u32) -> f32 {
    (height as f32 / (294.0 + race.runners.len() as f32 * 16.0).max(720.0)).max(0.1)
}

fn route(out: &mut Vec<UiVertex>, points: &[[f32; 3]], p: Projector, width: u32, height: u32) {
    for pair in points.windows(2) {
        if let Some((a, b)) = p.clip_line(
            p.project(Vec3::from_array(pair[0])).truncate(),
            p.project(Vec3::from_array(pair[1])).truncate(),
        ) {
            line(
                out,
                a,
                b,
                p.ui_scale,
                [0.55, 0.82, 0.9, 0.65],
                width,
                height,
            );
        }
    }
}

fn triangle(out: &mut Vec<UiVertex>, points: [Vec2; 3], color: [f32; 4], width: u32, height: u32) {
    out.extend(points.map(|p| UiVertex {
        position: [
            p.x / width.max(1) as f32 * 2.0 - 1.0,
            1.0 - p.y / height.max(1) as f32 * 2.0,
        ],
        uv: [0.0; 2],
        color,
        textured: 0.0,
    }));
}

fn line(
    out: &mut Vec<UiVertex>,
    a: Vec2,
    b: Vec2,
    thickness: f32,
    color: [f32; 4],
    width: u32,
    height: u32,
) {
    let delta = b - a;
    if !delta.is_finite() || delta.length_squared() < 0.01 {
        return;
    }
    let n = Vec2::new(-delta.y, delta.x).normalize() * thickness * 0.5;
    triangle(out, [a - n, a + n, b + n], color, width, height);
    triangle(out, [a - n, b + n, b - n], color, width, height);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn fixture() -> StudioRaceUi {
        // Deliberately disconnected triangles exercise the worst outline count.
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        for i in 0..MAX_MAP_TRIANGLES {
            let x = (i % 32) as f32 * 10.0;
            let y = (i / 32) as f32 * 10.0;
            let a = vertices.len() as u32;
            vertices.extend([[x, y, 0.0], [x + 20.0, y, 0.0], [x, y + 20.0, 20.0]]);
            triangles.push([a, a + 1, a + 2]);
        }
        let route: Vec<_> = (0..300).map(|i| [i as f32, i as f32, 20.0]).collect();
        let map = Arc::new(
            StudioRaceMap::new(vertices, triangles, route.clone(), &route)
                .with_routes((0..32).map(|_| route.clone()).collect()),
        );
        StudioRaceUi {
            section_clock: true,
            phase: "SECTION REPLAY 1/3 | runner | best of 32 | real speed".into(),
            section: 1,
            runners: (0..32)
                .map(|i| super::super::StudioRaceRunnerUi {
                    route_index: i,
                    rank: i + 1,
                    gap: Some(i as f64 * 0.2),
                    focused: i == 0,
                    name: "abcdefghijklmnopq".into(),
                    color: [30, 220, 255],
                    position: [i as f32 * 5.0, 0.0, 20.0],
                    elapsed: 123.45,
                    finished: true,
                    reference: i == 0,
                })
                .collect(),
            route: Vec::new(),
            map: Some(map),
            output_time: 3.0,
            font: String::new(),
            readout_scale: 1.0,
        }
    }

    #[test]
    fn route_map_leaves_room_for_all_32_runners_and_world_labels_in_existing_hud_buffer() {
        let race = fixture();
        let mut out = Vec::new();
        // Real font draws include a shadow pass. Exercise full-length names
        // rather than undercounting the bitmap fallback's single pass.
        let glyph = super::super::ProportionalGlyph {
            width: 4,
            height: 9,
            horiz_advance: 4,
            baseline: 9,
            ..Default::default()
        };
        let font = super::super::ProportionalFont {
            glyphs: [glyph; 256],
            point_size: 9,
            height: 9,
        };
        super::super::studio_race::draw(&mut out, &race, Some(&font), 1280, 720);
        // Existing 1 MiB transient buffer. World names are capped at 16
        // characters, with shadowed glyphs and reference/focus prefixes.
        let bytes = (out.len() + (32 * 16 + 30) * 12) * std::mem::size_of::<UiVertex>();
        assert!(bytes < 1024 * 1024, "HUD requires {bytes} bytes");
        assert!(out
            .iter()
            .all(|v| v.position.iter().all(|x| x.is_finite() && x.abs() <= 1.0)));
        let map = race.map.as_ref().unwrap();
        assert!(map.edges.len() <= MAX_MAP_EDGES);
        assert!(!map.edges.is_empty());
        assert!(map
            .cache
            .lock()
            .unwrap()
            .vertices
            .iter()
            .all(|v| v.color[3] == 0.5));
    }

    #[test]
    fn local_map_moves_with_new_focus_and_clips_geometry_to_its_panel() {
        let mut race = fixture();
        draw(&mut Vec::new(), &race, 1280, 720);
        let map = Arc::clone(race.map.as_ref().unwrap());
        let initial = map.cache.lock().unwrap().key;
        race.runners[0].focused = false;
        race.runners[1].focused = true;
        race.runners[1].position = [1000.0, 2000.0, 3000.0];
        draw(&mut Vec::new(), &race, 1280, 720);
        assert_ne!(
            initial,
            map.cache.lock().unwrap().key,
            "changing focus must reframe the map even at the same timestamp"
        );
        let p = Projector::new(Vec3::ZERO, Vec2::splat(2300.0), 0, 1280, 1.0);
        let (low, high) = p.bounds();
        let (a, b) = p
            .clip_line(low - Vec2::splat(100.0), high + Vec2::splat(100.0))
            .unwrap();
        for point in [a, b] {
            assert!(
                point.cmpge(low - Vec2::splat(0.01)).all()
                    && point.cmple(high + Vec2::splat(0.01)).all()
            );
        }
    }

    #[test]
    fn route_map_scrubbing_is_deterministic_and_cached_players_keep_moving() {
        let mut race = fixture();
        let mut before = Vec::new();
        draw(&mut before, &race, 1280, 720);
        let map = Arc::clone(race.map.as_ref().unwrap());
        let (key, pointer) = {
            let c = map.cache.lock().unwrap();
            (c.key, c.vertices.as_ptr())
        };
        race.output_time += 0.001;
        race.runners[0].position[0] += 30.0;
        let mut after = Vec::new();
        draw(&mut after, &race, 1280, 720);
        assert_ne!(
            bytemuck::cast_slice::<_, u8>(&before),
            bytemuck::cast_slice::<_, u8>(&after)
        );
        {
            let c = map.cache.lock().unwrap();
            assert_eq!(c.key, key);
            assert_eq!(c.vertices.as_ptr(), pointer);
        }
        race.output_time = 30.0;
        draw(&mut Vec::new(), &race, 1280, 720);
        race.output_time = 3.0;
        race.runners[0].position[0] -= 30.0;
        let mut scrubbed = Vec::new();
        draw(&mut scrubbed, &race, 1280, 720);
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&before),
            bytemuck::cast_slice::<_, u8>(&scrubbed)
        );
    }
}

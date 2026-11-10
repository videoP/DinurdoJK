//! cg_specHudMap: the broadcast radar. A top-down floor plan of the current
//! map (walkable surfaces shaded by height, ledge outlines) with team blips.
//! The plan is built once per map on a worker (`app::spectator_broadcast_map`)
//! and drawn in the transient HUD batch: no world render, textures or GPU
//! resources. Projected floor vertices are cached between frames.
use super::{disc, UiVertex};
use glam::{Vec2, Vec3};
use std::sync::{Arc, Mutex};

/// Draw budget for the floor plan (each triangle is 3 HUD vertices).
pub(crate) const MAX_RADAR_TRIANGLES: usize = 2000;
pub(crate) const MAX_RADAR_EDGES: usize = 700;
/// Follow view: world units from the observed player to the panel edge.
const FOLLOW_RADIUS: f32 = 1600.0;
/// Follow view centre quantization (world units); keeps the cache warm.
const FOLLOW_STEP: f32 = 8.0;

const FLOOR_LOW: [f32; 3] = [0.10, 0.12, 0.155];
const FLOOR_HIGH: [f32; 3] = [0.44, 0.48, 0.55];
const EDGE_LOW: [f32; 3] = [0.40, 0.45, 0.52];
const EDGE_HIGH: [f32; 3] = [0.78, 0.82, 0.88];

#[derive(Debug, Clone, Copy)]
enum Shape {
    Tri([Vec2; 3]),
    Edge([Vec2; 2]),
}

#[derive(Debug)]
pub struct BroadcastMinimap {
    /// Floors and outlines in world XY, sorted low to high so upper floors
    /// cover lower ones and each outline lands over the floor it bounds.
    shapes: Vec<(f32, Shape)>,
    low: Vec3,
    high: Vec3,
    cache: Mutex<Cache>,
}

#[derive(Debug, Default)]
struct Cache {
    key: Option<(u32, u32, [i32; 4], [i32; 2])>,
    vertices: Vec<UiVertex>,
}

/// Arc identity wrapper so `SpectatorBroadcastUi` keeps `Eq`.
#[derive(Debug, Clone)]
pub struct MinimapHandle(pub Arc<BroadcastMinimap>);

impl PartialEq for MinimapHandle {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for MinimapHandle {}

impl BroadcastMinimap {
    /// `triangles`: upward-facing floors; `edges`: their open boundaries.
    /// Both already clipped to `low..high`.
    pub(crate) fn new(triangles: Vec<[Vec3; 3]>, edges: Vec<[Vec3; 2]>, low: Vec3, high: Vec3) -> Self {
        let mut shapes: Vec<(f32, Shape)> = triangles.iter()
            .map(|t| ((t[0].z + t[1].z + t[2].z) / 3.0, Shape::Tri(t.map(Vec3::truncate))))
            .chain(edges.iter().map(|e| (e[0].z.max(e[1].z) + 0.5, Shape::Edge(e.map(Vec3::truncate)))))
            .collect();
        shapes.sort_by(|a, b| a.0.total_cmp(&b.0));
        Self { shapes, low, high, cache: Mutex::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    /// World XY extent of the overview.
    pub fn extent(&self) -> Vec2 {
        (self.high - self.low).truncate().max(Vec2::ONE)
    }

    fn shade(&self, z: f32, low: [f32; 3], high: [f32; 3], alpha: f32) -> [f32; 4] {
        let t = ((z - self.low.z) / (self.high.z - self.low.z).max(1.0)).clamp(0.0, 1.0);
        // Ease so mid levels separate more than the extremes.
        let t = t * t * (3.0 - 2.0 * t);
        [
            low[0] + (high[0] - low[0]) * t,
            low[1] + (high[1] - low[1]) * t,
            low[2] + (high[2] - low[2]) * t,
            alpha,
        ]
    }
}

/// What the panel shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum RadarView {
    /// The whole map, north up.
    Overview,
    /// A fixed-radius window that follows a world point, north up.
    Follow(Vec2),
}

/// World XY -> screen for one panel.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RadarFrame {
    /// Inner panel rectangle in screen pixels.
    pub rect: [f32; 4],
    /// World point at the rectangle's top-left corner (min x, max y).
    origin: Vec2,
    scale: f32,
}

impl RadarFrame {
    pub fn to_screen(&self, world: Vec2) -> Vec2 {
        Vec2::new(
            self.rect[0] + (world.x - self.origin.x) * self.scale,
            self.rect[1] + (self.origin.y - world.y) * self.scale,
        )
    }

    fn low(&self) -> Vec2 {
        Vec2::new(self.rect[0], self.rect[1])
    }

    fn high(&self) -> Vec2 {
        Vec2::new(self.rect[0] + self.rect[2], self.rect[1] + self.rect[3])
    }

    /// Inside the panel, inset by `inset` pixels.
    pub fn contains(&self, p: Vec2, inset: f32) -> bool {
        p.cmpge(self.low() + Vec2::splat(inset)).all() && p.cmple(self.high() - Vec2::splat(inset)).all()
    }

    pub fn clamp(&self, p: Vec2, inset: f32) -> Vec2 {
        p.clamp(self.low() + Vec2::splat(inset), self.high() - Vec2::splat(inset))
    }
}

/// One player (or the free camera) on the radar.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RadarBlip {
    pub position: Vec3,
    /// Degrees, JKA yaw (0 = +X, counter-clockwise).
    pub yaw: f32,
    pub color: [f32; 4],
    pub observed: bool,
    pub alive: bool,
    /// Last known position, not currently presented.
    pub stale: bool,
    /// Carried flag's colour.
    pub flag: Option<[f32; 4]>,
    pub camera: bool,
}

/// Panel content size in pixels for `view` inside `max` (aspect kept).
pub(crate) fn panel_size(map: &BroadcastMinimap, view: RadarView, max: Vec2) -> Vec2 {
    match view {
        RadarView::Follow(_) => Vec2::splat(max.x.min(max.y)),
        RadarView::Overview => {
            let extent = map.extent();
            let scale = (max.x / extent.x).min(max.y / extent.y);
            (extent * scale).max(Vec2::splat(max.min_element() * 0.4))
        }
    }
}

/// Floor plan into `rect`; returns the frame blips must use.
pub(crate) fn draw_floor(
    out: &mut Vec<UiVertex>,
    map: &BroadcastMinimap,
    rect: [f32; 4],
    view: RadarView,
    s: f32,
    width: u32,
    height: u32,
) -> RadarFrame {
    let (frame, center_key, clip) = match view {
        RadarView::Overview => {
            let extent = map.extent();
            let scale = (rect[2] / extent.x).min(rect[3] / extent.y);
            // Centre the plan in the rectangle.
            let used = extent * scale;
            let offset = (Vec2::new(rect[2], rect[3]) - used) * 0.5 / scale;
            let origin = Vec2::new(map.low.x - offset.x, map.high.y + offset.y);
            (RadarFrame { rect, origin, scale }, [0, 0], false)
        }
        RadarView::Follow(center) => {
            let cell = (center / FOLLOW_STEP).round();
            let center = cell * FOLLOW_STEP;
            let scale = rect[2].min(rect[3]) / (2.0 * FOLLOW_RADIUS);
            let half = Vec2::new(rect[2], rect[3]) * 0.5 / scale;
            let origin = Vec2::new(center.x - half.x, center.y + half.y);
            (RadarFrame { rect, origin, scale }, [cell.x as i32, cell.y as i32], true)
        }
    };
    let rect_key = rect.map(|v| (v * 4.0).round() as i32);
    let key = (width, height, rect_key, center_key);
    let mut cache = map.cache.lock().unwrap_or_else(|e| e.into_inner());
    if cache.key != Some(key) {
        cache.vertices.clear();
        let thickness = (1.1 * s).max(1.0);
        let mut polygon = Vec::with_capacity(9);
        for &(z, shape) in &map.shapes {
            match shape {
                Shape::Tri(t) => {
                    let color = map.shade(z, FLOOR_LOW, FLOOR_HIGH, 0.94);
                    let points = t.map(|p| frame.to_screen(p));
                    if !clip {
                        triangle(&mut cache.vertices, points, color, width, height);
                        continue;
                    }
                    clip_polygon(&points, frame.low(), frame.high(), &mut polygon);
                    for i in 1..polygon.len().saturating_sub(1) {
                        triangle(&mut cache.vertices, [polygon[0], polygon[i], polygon[i + 1]], color, width, height);
                    }
                }
                Shape::Edge(e) => {
                    let color = map.shade(z, EDGE_LOW, EDGE_HIGH, 0.7);
                    let (a, b) = (frame.to_screen(e[0]), frame.to_screen(e[1]));
                    let clipped = if clip { clip_line(a, b, frame.low(), frame.high()) } else { Some((a, b)) };
                    if let Some((a, b)) = clipped {
                        line(&mut cache.vertices, a, b, thickness, color, width, height);
                    }
                }
            }
        }
        cache.key = Some(key);
    }
    out.extend_from_slice(&cache.vertices);
    frame
}

/// Team blips over the floor plan; the observed player last (on top).
pub(crate) fn draw_blips(
    out: &mut Vec<UiVertex>,
    frame: &RadarFrame,
    blips: &[RadarBlip],
    s: f32,
    width: u32,
    height: u32,
) {
    let mut order: Vec<&RadarBlip> = blips.iter().collect();
    order.sort_by_key(|blip| (blip.camera || blip.observed, blip.alive));
    for blip in order {
        let raw = frame.to_screen(blip.position.truncate());
        if !raw.is_finite() {
            continue;
        }
        let edge = 6.0 * s;
        let pinned = !frame.contains(raw, edge);
        let p = frame.clamp(raw, edge);
        let fade = if blip.stale { 0.45 } else { 1.0 };
        let with_alpha = |color: [f32; 4], alpha: f32| [color[0], color[1], color[2], color[3] * alpha * fade];
        if !blip.alive && !blip.camera {
            // A dead player: a small cross where they fell.
            let r = 3.6 * s;
            let color = with_alpha(blip.color, 0.75);
            line(out, p - Vec2::splat(r), p + Vec2::splat(r), 1.6 * s, color, width, height);
            line(out, p + Vec2::new(-r, r), p + Vec2::new(r, -r), 1.6 * s, color, width, height);
            continue;
        }
        let radius = if pinned { 3.2 } else if blip.observed { 5.6 } else { 4.4 } * s;
        if !pinned {
            // View cone: screen Y grows down, world Y grows up.
            let yaw = blip.yaw.to_radians();
            let spread = if blip.camera { 0.62 } else { 0.5 };
            let reach = radius * if blip.camera { 4.2 } else { 3.4 };
            let ray = |angle: f32| p + Vec2::new(angle.cos(), -angle.sin()) * reach;
            let cone = with_alpha(if blip.camera { [1.0, 1.0, 1.0, 1.0] } else { blip.color }, if blip.observed || blip.camera { 0.42 } else { 0.26 });
            triangle(out, [p, ray(yaw - spread), ray(yaw + spread)], cone, width, height);
        }
        if blip.camera {
            disc(out, p.x, p.y, 2.6 * s, 10, with_alpha([1.0, 1.0, 1.0, 1.0], 0.95), width, height);
            continue;
        }
        if blip.observed {
            disc(out, p.x, p.y, radius + 2.4 * s, 18, with_alpha([1.0, 1.0, 1.0, 1.0], 0.95), width, height);
        }
        disc(out, p.x, p.y, radius + 1.3 * s, 16, with_alpha([0.01, 0.015, 0.02, 1.0], 0.9), width, height);
        disc(out, p.x, p.y, radius, 16, with_alpha(blip.color, 1.0), width, height);
        if let Some(flag) = blip.flag {
            // A pennant on a short pole above the blip.
            let base = p + Vec2::new(radius * 0.6, -radius * 0.6);
            let top = base - Vec2::Y * 9.0 * s;
            line(out, base, top, 1.3 * s, with_alpha([0.95, 0.95, 0.95, 1.0], 1.0), width, height);
            triangle(out, [top, top + Vec2::new(7.0 * s, 2.6 * s), top + Vec2::Y * 5.2 * s], with_alpha(flag, 1.0), width, height);
        }
    }
}

fn clip_polygon(points: &[Vec2; 3], low: Vec2, high: Vec2, out: &mut Vec<Vec2>) {
    out.clear();
    out.extend_from_slice(points);
    let mut next = Vec::with_capacity(9);
    for axis in 0..2 {
        for (bound, sign) in [(low[axis], 1.0), (high[axis], -1.0)] {
            next.clear();
            let Some(&last) = out.last() else { return };
            let mut a = last;
            let mut da = (a[axis] - bound) * sign;
            for &b in out.iter() {
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
            std::mem::swap(out, &mut next);
        }
    }
}

fn clip_line(a: Vec2, b: Vec2, low: Vec2, high: Vec2) -> Option<(Vec2, Vec2)> {
    let d = b - a;
    let (mut begin, mut end) = (0.0_f32, 1.0_f32);
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

fn triangle(out: &mut Vec<UiVertex>, points: [Vec2; 3], color: [f32; 4], width: u32, height: u32) {
    let color = UiVertex::pack_color(color);
    out.extend(points.map(|p| UiVertex {
        position: [p.x / width.max(1) as f32 * 2.0 - 1.0, 1.0 - p.y / height.max(1) as f32 * 2.0],
        uv: [0.0; 2],
        color,
        textured: 0.0,
    }));
}

fn line(out: &mut Vec<UiVertex>, a: Vec2, b: Vec2, thickness: f32, color: [f32; 4], width: u32, height: u32) {
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

    fn square_map() -> BroadcastMinimap {
        let tri = |z: f32| [Vec3::new(0.0, 0.0, z), Vec3::new(1000.0, 0.0, z), Vec3::new(0.0, 1000.0, z)];
        BroadcastMinimap::new(vec![tri(100.0), tri(0.0)], vec![[Vec3::new(0.0, 0.0, 0.0), Vec3::new(1000.0, 0.0, 0.0)]],
            Vec3::new(0.0, 0.0, 0.0), Vec3::new(1000.0, 1000.0, 100.0))
    }

    #[test]
    fn floors_draw_low_to_high() {
        let map = square_map();
        let heights: Vec<f32> = map.shapes.iter().map(|s| s.0).collect();
        assert!(heights.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn overview_is_north_up_and_fits_the_rect() {
        let map = square_map();
        let mut out = Vec::new();
        let frame = draw_floor(&mut out, &map, [10.0, 20.0, 200.0, 200.0], RadarView::Overview, 1.0, 1920, 1080);
        let north_west = frame.to_screen(Vec2::new(0.0, 1000.0));
        let south_east = frame.to_screen(Vec2::new(1000.0, 0.0));
        assert!((north_west - Vec2::new(10.0, 20.0)).length() < 0.01);
        assert!((south_east - Vec2::new(210.0, 220.0)).length() < 0.01);
        assert!(out.iter().all(|v| v.position.iter().all(|x| x.is_finite() && x.abs() <= 1.0)));
    }

    #[test]
    fn follow_view_clips_to_its_panel() {
        let map = square_map();
        let mut out = Vec::new();
        let rect = [100.0, 100.0, 50.0, 50.0];
        draw_floor(&mut out, &map, rect, RadarView::Follow(Vec2::new(500.0, 500.0)), 1.0, 1000, 1000);
        for v in &out {
            let x = (v.position[0] + 1.0) * 0.5 * 1000.0;
            let y = (1.0 - v.position[1]) * 0.5 * 1000.0;
            assert!((99.9..=150.1).contains(&x) && (99.9..=150.1).contains(&y), "{x} {y}");
        }
    }
}

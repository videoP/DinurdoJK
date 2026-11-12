//! Option A acceptance scenes on synthetic collision, run end to end through
//! the stage 5/6 CPU reference (`jka_water::river::capture::geometry`): a
//! cell-centre raster stands in for the five GPU depth captures.

use super::*;
use jka_assets::bsp::river::{CaptureFace, CaptureMesh};
use jka_water::river::capture::{geometry, Camera, Contours, Settings, CHANNELS};
use jka_water::river::CENTIMETRES_PER_MAP_UNIT as CM;

const CELL: f32 = 32.0;
const FLOOR_CM: f32 = -50_000.0;

fn region(dims: [u32; 2]) -> Region {
    Region { min: [0.0, 0.0], cell: f64::from(CELL), dims }
}

fn centre(x: u32, y: u32) -> [f32; 2] {
    [(x as f32 + 0.5) * CELL, (y as f32 + 0.5) * CELL]
}

/// A brush from its outward planes; faces and footprint from its corners.
fn brush(g: &mut CaptureGeometry, mins: [f32; 3], maxs: [f32; 3], bottom: Option<([f32; 3], f32)>) {
    let mut planes = vec![
        ([1.0, 0.0, 0.0], maxs[0]),
        ([-1.0, 0.0, 0.0], -mins[0]),
        ([0.0, 1.0, 0.0], maxs[1]),
        ([0.0, -1.0, 0.0], -mins[1]),
        ([0.0, 0.0, 1.0], maxs[2]),
    ];
    planes.push(bottom.unwrap_or(([0.0, 0.0, -1.0], -mins[2])));
    let (n, d) = planes[5];
    let under = |x: f32, y: f32| (d - n[0] * x - n[1] * y) / n[2];
    let corners = [[mins[0], mins[1]], [maxs[0], mins[1]], [maxs[0], maxs[1]], [mins[0], maxs[1]]];
    let top = corners.map(|c| [c[0], c[1], maxs[2]]);
    let low = corners.map(|c| [c[0], c[1], under(c[0], c[1])]);
    let start = g.brush_faces.len() as u32;
    for (q, up) in [(top, true), (low, false)] {
        for t in [[q[0], q[1], q[2]], [q[0], q[2], q[3]]] {
            g.brush_faces.push(CaptureFace { triangle: t, up, sky: false });
        }
    }
    let zmin = low.iter().map(|p| p[2]).fold(f32::INFINITY, f32::min);
    g.brushes.push(CaptureBrush {
        planes,
        mins: [mins[0], mins[1], zmin],
        maxs,
        footprint: corners.to_vec(),
        faces: start..g.brush_faces.len() as u32,
    });
}

fn slab(g: &mut CaptureGeometry, x: [f32; 2], y: [f32; 2], z: [f32; 2]) {
    brush(g, [x[0], y[0], z[0]], [x[1], y[1], z[1]], None);
}

fn z_at(t: &[[f32; 3]; 3], p: [f32; 2]) -> Option<f32> {
    let d = (t[1][1] - t[2][1]) * (t[0][0] - t[2][0]) + (t[2][0] - t[1][0]) * (t[0][1] - t[2][1]);
    if d.abs() < 1e-9 {
        return None;
    }
    let a = ((t[1][1] - t[2][1]) * (p[0] - t[2][0]) + (t[2][0] - t[1][0]) * (p[1] - t[2][1])) / d;
    let b = ((t[2][1] - t[0][1]) * (p[0] - t[2][0]) + (t[0][0] - t[2][0]) * (p[1] - t[2][1])) / d;
    let c = 1.0 - a - b;
    (a >= -1e-5 && b >= -1e-5 && c >= -1e-5).then(|| a * t[0][2] + b * t[1][2] + c * t[2][2])
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-2
}

struct Scene {
    classified: Classified,
    region: Region,
    /// Selected bed per cell (map units), stage 6 with the flood reference.
    bottom: Vec<f32>,
    under: Vec<f32>,
}

impl Scene {
    fn bed(&self, x: u32, y: u32) -> f32 {
        self.bottom[(y * self.region.dims[0] + x) as usize]
    }
}

fn run(g: &CaptureGeometry, dims: [u32; 2], source: [u32; 2], ground: f32, level: f32) -> Scene {
    let region = region(dims);
    let p = centre(source[0], source[1]);
    let classified = classify(g, region, [p[0], p[1], ground + 2.0], level).expect("classify");
    let mut contours = Vec::new();
    let mut water = Vec::new();
    let mut under_map = Vec::new();
    for y in 0..dims[1] {
        for x in 0..dims[0] {
            let p = centre(x, y);
            let max = |tris: &[[[f32; 3]; 3]]| tris.iter().filter_map(|t| z_at(t, p)).fold(f32::NEG_INFINITY, f32::max);
            let landscape = max(&classified.landscape);
            let top = max(&classified.actors);
            let under = classified.actors.iter().filter_map(|t| z_at(t, p)).fold(f32::INFINITY, f32::min);
            under_map.push(under);
            let cm = |z: f32, empty: f32| if z.is_finite() { z * CM } else { empty };
            contours.push(Contours {
                landscape: cm(landscape, FLOOR_CM),
                actors_top: cm(top, FLOOR_CM),
                actors_under: cm(under, 1.0e9),
                dilated_top: FLOOR_CM,
                dilated_under: 1.0e9,
                padding: [0.0; 3],
            });
            water.push(classified.references.local_cm([i64::from(x), i64::from(y)], 0.0, FLOOR_CM));
        }
    }
    let settings = Settings {
        dimensions: dims,
        world_size: dims.map(|n| n as f32 * CELL * CM),
        capture_width: dims[0].max(dims[1]) as f32 * CELL * CM,
        dx: CELL * CM,
        bottom_depth: FLOOR_CM,
        dilation: 0.0,
        cameras: [Camera { height: 0.0, near: 0.0, far: 1.0e7 }; CHANNELS],
    };
    let bottom = geometry(settings, &contours, &water).expect("stage 6").iter().map(|g| g.bottom / CM).collect();
    Scene { classified, region, bottom, under: under_map }
}

/// Upstream floor 900 drops to a long downstream floor at 400; the source
/// level 1000 is far above everything downstream.
fn downhill(g: &mut CaptureGeometry) {
    slab(g, [0.0, 256.0], [0.0, 256.0], [-64.0, 900.0]);
    slab(g, [256.0, 768.0], [0.0, 256.0], [-64.0, 400.0]);
}

#[test]
fn downhill_bridge_below_source_level_is_passed_under() {
    let mut g = CaptureGeometry::default();
    downhill(&mut g);
    slab(&mut g, [448.0, 576.0], [0.0, 256.0], [600.0, 640.0]);
    let s = run(&g, [24, 8], [2, 4], 900.0, 1000.0);
    for x in 14..18 {
        assert_eq!(s.bed(x, 4), 400.0, "bed under the deck at cell {x}");
    }
    assert_eq!(s.bed(2, 4), 900.0);
    assert_eq!(s.bed(20, 4), 400.0);
    assert_eq!(s.classified.stats.actor_items, 1);
    assert_eq!(s.classified.stats.stacked, 0);
    // Legacy rule for contrast: the deck's bottom (600) is below the source
    // level, so mode 0 kept it as ground and the river stopped at the bridge.
    assert!(crate::river_capture::clip_collision(&[[448.0, 0.0, 640.0], [576.0, 0.0, 640.0], [576.0, 256.0, 640.0]], Some(600.0), Some(1000.0)).is_some());
}

#[test]
fn pier_under_a_bridge_stays_an_obstacle() {
    let mut g = CaptureGeometry::default();
    downhill(&mut g);
    slab(&mut g, [448.0, 576.0], [0.0, 256.0], [600.0, 640.0]);
    slab(&mut g, [488.0, 504.0], [104.0, 120.0], [360.0, 600.0]);
    let s = run(&g, [24, 8], [2, 4], 900.0, 1000.0);
    assert!(s.bed(15, 3) >= 600.0, "pier cell bed {}", s.bed(15, 3));
    for (x, y) in [(15, 2), (15, 4), (14, 3), (16, 3)] {
        assert_eq!(s.bed(x, y), 400.0, "water passes around the pier at ({x},{y})");
    }
}

#[test]
fn grounded_dam_is_a_barrier_without_an_invented_underpass() {
    let mut g = CaptureGeometry::default();
    downhill(&mut g);
    slab(&mut g, [320.0, 352.0], [0.0, 256.0], [300.0, 700.0]);
    let s = run(&g, [24, 8], [2, 4], 900.0, 1000.0);
    for y in 0..8 {
        assert_eq!(s.bed(10, y), 700.0);
    }
    assert_eq!(s.classified.stats.actor_items, 0);
}

#[test]
fn sealed_subfloor_is_never_selected() {
    let mut g = CaptureGeometry::default();
    slab(&mut g, [0.0, 768.0], [0.0, 256.0], [-64.0, 150.0]);
    slab(&mut g, [0.0, 768.0], [0.0, 256.0], [380.0, 400.0]);
    let s = run(&g, [24, 8], [2, 4], 400.0, 500.0);
    assert!(s.bottom.iter().all(|&b| near(b, 400.0)), "{:?}", s.bottom);
    assert_eq!(s.classified.stats.actor_items, 0);
}

#[test]
fn open_tunnel_passes_water_under_its_roof() {
    let mut g = CaptureGeometry::default();
    slab(&mut g, [0.0, 768.0], [0.0, 256.0], [-64.0, 400.0]);
    slab(&mut g, [384.0, 640.0], [0.0, 256.0], [600.0, 1200.0]);
    let s = run(&g, [24, 8], [2, 4], 400.0, 1000.0);
    assert!(s.bottom.iter().all(|&b| near(b, 400.0)));
    assert_eq!(s.classified.stats.actor_items, 1);
}

#[test]
fn overhead_only_ground_stays_void() {
    let mut g = CaptureGeometry::default();
    slab(&mut g, [0.0, 384.0], [0.0, 256.0], [-64.0, 400.0]);
    // A deck over the void beyond the floor's end: no ground under it.
    slab(&mut g, [384.0, 512.0], [0.0, 256.0], [600.0, 640.0]);
    let s = run(&g, [24, 8], [2, 4], 400.0, 1000.0);
    for x in (12..15).chain(16..24) {
        assert_eq!(s.bed(x, 4), FLOOR_CM / CM, "void at cell {x}");
    }
    assert_eq!(s.bed(11, 4), 400.0);
    assert!(s.classified.stats.voids > 0);
    assert_eq!(s.classified.stats.actor_items, 1);
}

#[test]
fn brush_only_floor_is_landscape() {
    let mut g = CaptureGeometry::default();
    slab(&mut g, [0.0, 768.0], [0.0, 256.0], [-64.0, 400.0]);
    let s = run(&g, [24, 8], [2, 4], 400.0, 1000.0);
    assert!(s.bottom.iter().all(|&b| near(b, 400.0)));
    assert!(s.classified.actors.is_empty());
    assert_eq!(s.classified.landscape.len(), 2);
}

#[test]
fn sloping_underside_comes_from_its_real_faces() {
    let mut g = CaptureGeometry::default();
    downhill(&mut g);
    // Bottom z = 600 + (x - 448) / 4: lowest point 600, 632 at the far end.
    let len = (1.0f32 + 1.0 / 16.0).sqrt();
    let plane = ([0.25 / len, 0.0, -1.0 / len], (0.25 * 448.0 - 600.0) / len);
    brush(&mut g, [448.0, 0.0, 600.0], [576.0, 256.0, 700.0], Some(plane));
    let s = run(&g, [24, 8], [2, 4], 900.0, 1000.0);
    let i = (4 * 24 + 16) as usize; // centre x = 528
    assert!((s.under[i] - 620.0).abs() < 1e-3, "underside {}", s.under[i]);
    assert_eq!(s.bed(16, 4), 400.0);
}

#[test]
fn zero_thickness_arch_and_thin_railing_become_actors() {
    let mut g = CaptureGeometry::default();
    slab(&mut g, [0.0, 768.0], [0.0, 256.0], [-64.0, 400.0]);
    for t in [[[448.0, 0.0, 620.0], [576.0, 0.0, 620.0], [576.0, 256.0, 620.0]], [[448.0, 0.0, 620.0], [576.0, 256.0, 620.0], [448.0, 256.0, 620.0]]] {
        g.meshes.push(CaptureMesh { triangle: t, face: MeshFace::Up });
    }
    // Railing post narrower than a cell, between cell centres.
    slab(&mut g, [500.0, 504.0], [100.0, 104.0], [620.0, 660.0]);
    let s = run(&g, [24, 8], [2, 4], 400.0, 1000.0);
    assert_eq!(s.bed(16, 4), 400.0);
    assert!(s.classified.actors.iter().any(|t| t[0][2] == 660.0), "railing top is an actor");
    assert!(s.classified.landscape.iter().all(|t| t[0][2] == 400.0));
}

#[test]
fn classification_is_independent_of_the_region_window() {
    let mut g = CaptureGeometry::default();
    slab(&mut g, [-64.0, 832.0], [-64.0, 320.0], [-64.0, 400.0]);
    slab(&mut g, [448.0, 576.0], [-64.0, 320.0], [600.0, 640.0]);
    let small = classify(&g, region([24, 8]), [80.0, 144.0, 402.0], 1000.0).unwrap();
    let large = classify(
        &g,
        Region { min: [-64.0, -64.0], cell: f64::from(CELL), dims: [28, 12] },
        [80.0, 144.0, 402.0],
        1000.0,
    )
    .unwrap();
    assert_eq!(small.stats.actor_items, large.stats.actor_items);
    for y in 0..8i64 {
        for x in 0..24i64 {
            assert_eq!(small.references.get([x, y]), large.references.get([x + 2, y + 2]));
        }
    }
}

#[test]
fn gaps_merge_touching_solids_and_bound_with_infinities() {
    let mut out = Vec::new();
    gaps(&mut [(380.0, 400.0, 0), (-64.0, 150.0, 1), (400.5, 410.0, 2)], &mut out);
    let pairs: Vec<_> = out.iter().map(|g| (g.floor, g.ceil)).collect();
    assert_eq!(pairs, [(f32::NEG_INFINITY, -64.0), (150.0, 380.0), (410.0, f32::INFINITY)]);
    out.clear();
    gaps(&mut [], &mut out);
    assert_eq!(out.len(), 1);
}

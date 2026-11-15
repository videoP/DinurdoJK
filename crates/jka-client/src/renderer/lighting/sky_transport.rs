//! White-sky transport for authored-room protection, independent of the live sun.
//! Two diffuse bounces, fixed reflectance, no artificial or baked emitters.
use super::{BakeProgress, Key, VertexSkySample};
use crate::renderer::{Arc, StaticAoBvh, StaticAoLightmapTriangle, Vec3};
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const PATHS: usize = 64;
const BOUNCES: usize = 2;
const REFLECTANCE: f32 = 0.6;
const MAX_DISTANCE: f32 = 131072.0;
const MAX_BYTES: u64 = 32 * 1024 * 1024;

pub(super) struct TransportBake {
    sizes: Vec<[u32; 2]>,
    normals: u8,
    visibility: crate::renderer::SunVisibilityMode,
    models: bool,
    tinted: bool,
    /// Original-resolution direct and bounced white-sky response. Invalid
    /// luxels are negative; shader reconstruction excludes them and other charts.
    pub pages: Vec<Vec<[f32; 2]>>,
    vertices: Vec<(u32, [f32; 2])>,
}

impl TransportBake {
    pub fn matches(&self, key: &Key) -> bool {
        self.sizes == key.sky_sizes && self.normals == key.normals
            && self.visibility == key.visibility && self.models == key.models
            && self.tinted == key.tinted
    }

    pub fn vertex_response(&self, gpu_index: u32) -> Option<[f32; 2]> {
        self.vertices.binary_search_by_key(&gpu_index, |v| v.0).ok().map(|i| self.vertices[i].1)
    }
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

fn cosine_direction(normal: Vec3, u: f32, phi: f32) -> Vec3 {
    let tangent = normal.cross(if normal.y.abs() < 0.9 { Vec3::Y } else { Vec3::X }).normalize();
    let bitangent = normal.cross(tangent);
    let radius = u.sqrt();
    (tangent * (radius * phi.cos()) + bitangent * (radius * phi.sin())
        + normal * (1.0 - u).sqrt()).normalize()
}

/// `surface`: None = cutout, Some(t) = accepted surface with transmission t.
/// Transmitting layers are processed in hit order, never BVH traversal order.
pub(super) fn sample(
    position: Vec3, normal: Vec3, seed: u32,
    solid: Option<&StaticAoBvh>, sky: Option<&StaticAoBvh>,
    bounce_normals: &[Vec3], surface: &impl Fn(usize, [f32; 3]) -> Option<f32>,
    cancel: &AtomicBool,
) -> [f32; 2] {
    let mut result = [0.0; 2];
    if normal.length_squared() < 0.5 || sky.is_none() { return result; }
    for path in 0..PATHS {
        if cancel.load(Ordering::Relaxed) { return result; }
        let mut origin = position + normal * 0.5;
        let mut direction = cosine_direction(normal,
            (path as f32 + 0.5) / PATHS as f32, path as f32 * 2.399_963_1);
        let mut energy = 1.0 / PATHS as f32;
        for bounce in 0..=BOUNCES {
            let mut reflected = None;
            // Bound transparent-layer traversal, including coincident layers.
            for _ in 0..16 {
                let sky_distance = sky.and_then(|bvh| bvh.trace(origin, direction, MAX_DISTANCE));
                let hit = solid.and_then(|bvh| bvh.trace_hit_filtered(
                    origin, direction, sky_distance.unwrap_or(MAX_DISTANCE),
                    |index, bary| surface(index, bary).is_some(),
                ));
                let Some((distance, index, bary)) = hit else {
                    if sky_distance.is_some() { result[usize::from(bounce != 0)] += energy; }
                    break;
                };
                let transmission = surface(index, bary).unwrap_or(0.0).clamp(0.0, 1.0);
                let point = origin + direction * distance;
                if transmission > 0.0 {
                    energy *= transmission;
                    if energy < 1e-6 { break; }
                    origin = point + direction * 0.01;
                    continue;
                }
                let n = bounce_normals[index];
                // Backfaces absorb; bouncing off the inside of BSP walls
                // would invent routes through sealed rooms.
                if n.dot(direction) < -1e-5 && bounce < BOUNCES {
                    reflected = Some((point, n));
                }
                break;
            }
            let Some((point, n)) = reflected else { break; };
            energy *= REFLECTANCE;
            let random = hash(seed ^ (path as u32).wrapping_mul(0x9e37_79b9)
                ^ (bounce as u32).wrapping_mul(0x85eb_ca6b));
            let u = ((random & 65535) as f32 + 0.5) / 65536.0;
            let phi = ((random >> 16) as f32 + 0.5) / 65536.0 * std::f32::consts::TAU;
            direction = cosine_direction(n, u, phi);
            origin = point + n * 0.05;
        }
    }
    result
}

/// Low-frequency sky dependence must not turn rare Monte Carlo bounce hits
/// into dark speckles on a room's authored lighting. Filter within each atlas
/// chart only; padding and adjacent packed surfaces never contribute.
fn filter_page(source: &[[f32; 2]], owners: &[u32], [w, h]: [u32; 2]) -> Vec<[f32; 2]> {
    const KERNEL: [f32; 5] = [1.0, 4.0, 6.0, 4.0, 1.0];
    source.par_iter().enumerate().map(|(i, &value)| {
        if value[0] < 0.0 || owners[i] == 0 { return value; }
        let x = (i % w as usize) as i32;
        let y = (i / w as usize) as i32;
        let mut response = [0.0; 2];
        let mut weights = 0.0;
        for dy in -2..=2 {
            for dx in -2..=2 {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 { continue; }
                let j = ny as usize * w as usize + nx as usize;
                if owners[j] != owners[i] || source[j][0] < 0.0 { continue; }
                let weight = KERNEL[(dx + 2) as usize] * KERNEL[(dy + 2) as usize];
                for c in 0..2 { response[c] += source[j][c] * weight; }
                weights += weight;
            }
        }
        response.map(|v| v / weights.max(1e-6))
    }).collect()
}

pub(super) fn bake(
    key: &Key, cached: Option<&Arc<TransportBake>>,
    receivers: &[StaticAoLightmapTriangle], solid: Option<&StaticAoBvh>, sky: Option<&StaticAoBvh>,
    bounce_normals: &[Vec3], surface: &(impl Fn(usize, [f32; 3]) -> Option<f32> + Sync),
    cancel: &AtomicBool, progress: &BakeProgress, vertex_sky: &[VertexSkySample],
) -> Option<Arc<TransportBake>> {
    if !key.sky_transport || cancel.load(Ordering::Relaxed) { return None; }
    if let Some(cached) = cached.filter(|cached| cached.matches(key)) {
        println!("[SKY TRANSPORT] reusing fixed white-sky transport (independent of sun/moon)");
        return Some(Arc::clone(cached));
    }
    let bytes = key.sky_sizes.iter().map(|&[w, h]| u64::from(w) * u64::from(h) * 8).sum::<u64>()
        + vertex_sky.len() as u64 * std::mem::size_of::<(u32, [f32; 2])>() as u64;
    if bytes > MAX_BYTES {
        eprintln!("[SKY TRANSPORT] skipped: {bytes} bytes exceeds the 32 MiB transport budget");
        return None;
    }
    let started = std::time::Instant::now();
    let samples = crate::renderer::static_ao::bvh::rasterize_lightmap_texels(receivers, &key.sky_sizes, 1);
    let total = samples.iter().map(|p| p.len() as u32).sum::<u32>().max(1);
    let done = AtomicU32::new(0);
    let pages: Vec<Vec<[f32; 2]>> = samples.par_iter().enumerate().map(|(page, samples)| {
        samples.par_iter().enumerate().map(|(i, receiver)| {
            if i % 64 == 0 {
                progress.fraction(835, 50, done.fetch_add(64, Ordering::Relaxed) + 64, total);
            }
            if cancel.load(Ordering::Relaxed) { return [-1.0; 2]; }
            receiver.map_or([-1.0; 2], |receiver| sample(
                Vec3::from_array(receiver.position), Vec3::from_array(receiver.normal).normalize_or_zero(),
                hash(page as u32) ^ i as u32, solid, sky, bounce_normals, surface, cancel,
            ))
        }).collect()
    }).collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    let mut charted = receivers.to_vec();
    for (t, chart) in charted.iter_mut().zip(super::triangle_charts(receivers)) { t.owner = chart; }
    let owners = crate::renderer::static_ao::bvh::rasterize_lightmap_owners(&charted, &key.sky_sizes, 1);
    let pages = pages.into_par_iter().zip(&owners).zip(&key.sky_sizes)
        .map(|((page, owners), &size)| filter_page(&page, owners, size)).collect();
    if cancel.load(Ordering::Relaxed) { return None; }
    let mut vertices: Vec<_> = vertex_sky.iter().map(|v| (v.gpu_index, v.transport)).collect();
    vertices.sort_by_key(|v| v.0);
    vertices.dedup_by_key(|v| v.0);
    println!("[SKY TRANSPORT] white sky, {PATHS} paths/luxel, {BOUNCES} bounces, reflectance {REFLECTANCE}, {:.2}s, {:.2} MiB; lightmaps and artificial emitters excluded",
        started.elapsed().as_secs_f64(), bytes as f64 / (1024.0 * 1024.0));
    Some(Arc::new(TransportBake {
        sizes: key.sky_sizes.clone(), normals: key.normals, visibility: key.visibility,
        models: key.models, tinted: key.tinted, pages, vertices,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::StaticAoRenderTriangle;

    fn quad(triangles: &mut Vec<StaticAoRenderTriangle>, p: [[f32; 3]; 4]) {
        for indices in [[0, 1, 2], [0, 2, 3]] {
            triangles.push(StaticAoRenderTriangle { positions: indices.map(|i| p[i]) });
        }
    }

    #[test]
    fn sky_transport_filter_keeps_rooms_and_atlas_islands_independent() {
        let source = vec![[0.0, 0.0], [0.0, 0.009375], [0.0, 0.0], [0.5, 0.2], [-1.0, -1.0]];
        let filtered = filter_page(&source, &[1, 1, 1, 2, 0], [5, 1]);
        assert_eq!(filtered[3], source[3], "outdoor chart cannot sample the room");
        assert_eq!(filtered[4], [-1.0; 2], "uncovered luxels remain invalid");
        assert!(filtered[..3].iter().all(|v| v[0] == 0.0 && v[1] < 0.009375),
            "isolated bounce hits are smoothed without borrowing outdoor light");
        assert_eq!(filter_page(&vec![[0.0, 0.0]; 25], &[1; 25], [5, 5]), vec![[0.0, 0.0]; 25]);
    }

    #[test]
    fn sky_transport_separates_direct_bounce_and_sealed_receivers() {
        let cancel = AtomicBool::new(false);
        let mut triangles = Vec::new();
        // Open-topped courtyard, normals inward; roof only over its left half.
        quad(&mut triangles, [[-10., 0., -10.], [-10., 0., 10.], [10., 0., 10.], [10., 0., -10.]]);
        quad(&mut triangles, [[-10., 0., -10.], [10., 0., -10.], [10., 10., -10.], [-10., 10., -10.]]);
        quad(&mut triangles, [[10., 0., 10.], [-10., 0., 10.], [-10., 10., 10.], [10., 10., 10.]]);
        quad(&mut triangles, [[-10., 0., 10.], [-10., 0., -10.], [-10., 10., -10.], [-10., 10., 10.]]);
        quad(&mut triangles, [[10., 0., -10.], [10., 0., 10.], [10., 10., 10.], [10., 10., -10.]]);
        quad(&mut triangles, [[-10., 10., -10.], [0., 10., -10.], [0., 10., 10.], [-10., 10., 10.]]);
        let normals: Vec<_> = triangles.iter().map(|t| {
            let p = t.positions.map(Vec3::from_array);
            (p[1] - p[0]).cross(p[2] - p[0]).normalize()
        }).collect();
        let solid = StaticAoBvh::build(Arc::new(triangles.clone())).unwrap();
        let mut sky_triangles = Vec::new();
        quad(&mut sky_triangles, [[-100., 20., -100.], [100., 20., -100.], [100., 20., 100.], [-100., 20., 100.]]);
        let sky = StaticAoBvh::build(Arc::new(sky_triangles)).unwrap();
        let surface = |_, _| Some(0.0);
        let open = sample(Vec3::new(5., 0., 0.), Vec3::Y, 1, Some(&solid), Some(&sky), &normals, &surface, &cancel);
        let shaded = sample(Vec3::new(-9.9, 4., 0.), Vec3::X, 1, Some(&solid), Some(&sky), &normals, &surface, &cancel);
        assert!(open[0] > 0.05);
        assert!(shaded[1] > 0.0, "covered courtyard wall receives bounced sky");
        assert!(open.iter().chain(&shaded).all(|v| v.is_finite() && *v >= 0.0));
        assert!(open.iter().sum::<f32>() <= 1.0 && shaded.iter().sum::<f32>() <= 1.0);
        quad(&mut triangles, [[0., 10., -10.], [10., 10., -10.], [10., 10., 10.], [0., 10., 10.]]);
        let mut closed_normals = normals;
        closed_normals.extend([Vec3::NEG_Y; 2]);
        let closed = StaticAoBvh::build(Arc::new(triangles)).unwrap();
        assert_eq!(sample(Vec3::new(5., 0., 0.), Vec3::Y, 1, Some(&closed), Some(&sky), &closed_normals, &surface, &cancel), [0.0; 2]);
        assert_eq!(sample(Vec3::ZERO, Vec3::Y, 1, None, None, &[], &surface, &cancel), [0.0; 2], "a BSP void is not a sky emitter");
    }

    #[test]
    fn sky_transport_handles_cutouts_transmission_and_cancellation() {
        let cancel = AtomicBool::new(false);
        let mut triangles = Vec::new();
        quad(&mut triangles, [[-1000., 10., -1000.], [1000., 10., -1000.], [1000., 10., 1000.], [-1000., 10., 1000.]]);
        let solid = StaticAoBvh::build(Arc::new(triangles)).unwrap();
        let mut sky_triangles = Vec::new();
        quad(&mut sky_triangles, [[-1000., 20., -1000.], [1000., 20., -1000.], [1000., 20., 1000.], [-1000., 20., 1000.]]);
        let sky = StaticAoBvh::build(Arc::new(sky_triangles)).unwrap();
        let normals = [Vec3::NEG_Y; 2];
        let run = |surface: &dyn Fn(usize, [f32; 3]) -> Option<f32>| {
            sample(Vec3::ZERO, Vec3::Y, 0, Some(&solid), Some(&sky), &normals, &surface, &cancel)
        };
        assert_eq!(run(&|_, _| None), [1.0, 0.0], "cutout does not block sky");
        assert_eq!(run(&|_, _| Some(0.5)), [0.5, 0.0], "transmission attenuates sky without making a diffuse emitter");
        assert_eq!(run(&|_, _| Some(0.0)), [0.0, 0.0], "opaque roof blocks sky");
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(run(&|_, _| None), [0.0, 0.0]);
    }
}

//! Camera-independent static sunlight in lightmap UV space. The existing AO
//! rasterizer supplies receivers; one cancellable worker traces static visibility.
//! The shared lightmap compositor owns the final subtraction/tone/sun result.
use crate::renderer::{
    Arc, DrawBatch, DrawClass, GpuImage, GpuVertex, Renderer, StaticAoBvh, StaticAoRenderTriangle,
    SunVisibilityMode, SunBakeStage, StaticAoTexelSample, TextureData, Vec3, scene,
};
use rayon::prelude::*;
use std::collections::HashMap;
use std::time::Instant;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc,
};

/// Choose a bake-only Rayon worker count without touching any of the game's
/// other thread pools. The automatic policy budgets for the main thread, the
/// render thread, one audio callback thread, this bake's coordinator(s), and
/// active background-game jobs, then reserves one more logical CPU's capacity.
/// This is a scheduling budget, not CPU affinity: the OS may still schedule
/// several threads on the same logical processor.
fn automatic_bake_workers(logical: usize, coordinators: usize, active_background: usize) -> usize {
    const MAIN_RENDER_AUDIO: usize = 3;
    const FREE_LOGICAL_CPU: usize = 1;
    logical.saturating_sub(
        MAIN_RENDER_AUDIO
            .saturating_add(coordinators)
            .saturating_add(active_background)
            .saturating_add(FREE_LOGICAL_CPU)
    ).max(1)
}

pub(super) fn bake_worker_count(requested: u8, coordinators: usize) -> usize {
    if requested != 0 {
        return usize::from(requested.min(32));
    }
    let logical = std::thread::available_parallelism().map_or(1, usize::from);
    // Main and render are already covered above, even if between tracked tasks.
    // Only count *active* additional game workers, not parked pool threads.
    let active_background = crate::thread_activity::snapshot().iter().filter(|entry| {
        entry.active && !matches!(
            entry.slot,
            crate::thread_activity::ThreadSlot::Main | crate::thread_activity::ThreadSlot::Render
        )
    }).count();
    automatic_bake_workers(logical, coordinators, active_background)
}

#[derive(Clone)]
pub(in crate::renderer) struct Geometry {
    vertices: Arc<Vec<GpuVertex>>,
    batches: Vec<DrawBatch>,
    textures: Arc<Vec<TextureData>>,
    receivers: Arc<Vec<crate::renderer::StaticAoLightmapTriangle>>,
    sun_receivers: Arc<Vec<crate::renderer::StaticAoLightmapTriangle>>,
    auto_receivers: Arc<Vec<crate::renderer::StaticAoLightmapTriangle>>,
    model_materials: Vec<bool>,
    /// The original world prefix has complete material stage lists. Inline
    /// instances currently copy only their selected lightmap/vertex receivers.
    world_batch_count: usize,
    /// For each immutable sun receiver vertex, the actual indexed draw vertex.
    /// Inline *N vertices live after the static-AO prefix.
    vertex_gpu_indices: Arc<Vec<u32>>,
}

impl Geometry {
    pub(in crate::renderer) fn new(
        vertices: Arc<Vec<GpuVertex>>,
        batches: Vec<DrawBatch>,
        textures: Arc<Vec<TextureData>>,
        materials: &scene::MaterialDebugInfo,
        world_batch_count: usize,
        vertex_gpu_indices: Vec<u32>,
    ) -> Self {
        debug_assert_eq!(vertices.len(), vertex_gpu_indices.len());
        let receivers = Arc::new(crate::renderer::collect_static_ao_lightmap_triangles(
            &vertices, &batches,
        ));
        // Curved BSP walls are frequently tessellated into hard-normal faces.
        // Only the static sun's receiver normals are reconstructed. Preserve
        // true polygon geometry for BVH visibility and preserve original AO,
        // map-light and authored lightmap inputs.
        let mut sun_receivers = (*receivers).clone();
        let smoothed_corners = smooth_sun_receiver_normals(&mut sun_receivers);
        let mut auto_receivers = (*receivers).clone();
        let auto_corners = auto_smooth_sun_receiver_normals(&mut auto_receivers);
        println!(
            "[SUN LIGHTMAP] receiver normals smoothed: shared-edge {smoothed_corners} corners, vertex auto-smooth {auto_corners} corners (r_bakedSunNormals 1/2), {} triangles",
            receivers.len()
        );
        let sun_receivers = Arc::new(sun_receivers);
        let auto_receivers = Arc::new(auto_receivers);
        let model_materials = batches
            .iter().enumerate()
            .map(|(index, b)| {
                // r_drawBSPModelSurfaces is a WORLD built-in-model switch.
                // Inline *N instances have their own entity visibility and
                // must not lose vertex sky illumination based on this flag.
                index < world_batch_count && materials
                    .entries
                    .get(b.material_debug_index as usize)
                    .is_some_and(|m| m.name.starts_with("models/"))
            })
            .collect();
        Self {
            vertices,
            batches,
            textures,
            receivers,
            sun_receivers,
            auto_receivers,
            model_materials,
            world_batch_count,
            vertex_gpu_indices: Arc::new(vertex_gpu_indices),
        }
    }

    /// r_bakedSunNormals: 0 authored vertex normals, 1 shared-edge smoothing,
    /// 2 vertex-position auto-smooth. Casters always use the real triangles.
    fn sun_receivers(&self, mode: u8) -> &[crate::renderer::StaticAoLightmapTriangle] {
        match mode {
            0 => &self.receivers,
            1 => &self.sun_receivers,
            _ => &self.auto_receivers,
        }
    }

    /// Per-luxel lightmap chart IDs at the ORIGINAL lightmap resolution, for
    /// the compositor's chart-aware reconstruction. 0 = no chart. Padding
    /// luxels inherit their single adjacent chart, as bilinear sampling expects.
    pub(in crate::renderer) fn lightmap_charts(&self, sizes: &[[u32; 2]]) -> Vec<Vec<u32>> {
        let mut charted = (*self.receivers).clone();
        for (triangle, chart) in charted.iter_mut().zip(triangle_charts(&self.receivers)) {
            triangle.owner = chart;
        }
        let mut charts = crate::renderer::static_ao::bvh::rasterize_lightmap_owners(&charted, sizes, 1);
        for (page, &[w, h]) in charts.iter_mut().zip(sizes) {
            crate::renderer::static_ao::bvh::dilate_static_ao_owners(page, w, h, 1);
        }
        charts
    }
}

/// Lightmap charts are the connected components of lightmap-UV space: the
/// triangles of one BSP surface share UV vertices; separately packed atlas
/// islands never do. Returns a 1-based chart per triangle.
fn triangle_charts(triangles: &[crate::renderer::StaticAoLightmapTriangle]) -> Vec<u32> {
    fn root(parent: &mut [usize], mut node: usize) -> usize {
        while parent[node] != node {
            parent[node] = parent[parent[node]];
            node = parent[node];
        }
        node
    }
    let mut parent: Vec<usize> = (0..triangles.len()).collect();
    let mut first: HashMap<(usize, [i64; 2]), usize> = HashMap::new();
    for (ti, triangle) in triangles.iter().enumerate() {
        for uv in triangle.uvs {
            let key = (triangle.texture, uv.map(|v| (f64::from(v) * 262144.0).round() as i64));
            let other = *first.entry(key).or_insert(ti);
            let a = root(&mut parent, other);
            let b = root(&mut parent, ti);
            if a != b { parent[b] = a; }
        }
    }
    let mut ids = HashMap::new();
    (0..triangles.len()).map(|ti| {
        let r = root(&mut parent, ti);
        let next = ids.len() as u32 + 1;
        *ids.entry(r).or_insert(next)
    }).collect()
}

/// q3map2-style smoothing angle: each corner averages the vertex normals of
/// every corner at the same welded position whose FACE normal lies within 45
/// degrees of its own face. No shared edge or material match is required, so
/// T-junctions, split charts and blended terrain shaders still smooth. There is
/// no transitive chaining: a 90-degree brush corner stays hard.
fn auto_smooth_sun_receiver_normals(
    triangles: &mut [crate::renderer::StaticAoLightmapTriangle],
) -> usize {
    let face_normals: Vec<Vec3> = triangles.iter().map(|t| {
        let p = t.positions.map(Vec3::from_array);
        let geometric = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
        let shading = t.normals.iter().fold(Vec3::ZERO, |s, &n| s + Vec3::from_array(n));
        if geometric.dot(shading) < 0.0 { -geometric } else { geometric }
    }).collect();
    let mut welded: HashMap<[i32; 3], Vec<(usize, usize)>> = HashMap::new();
    for (ti, triangle) in triangles.iter().enumerate() {
        if face_normals[ti].length_squared() < 0.5 { continue; }
        for corner in 0..3 {
            let key = triangle.positions[corner].map(|v| (v * 8.0).round() as i32);
            welded.entry(key).or_default().push((ti, corner));
        }
    }
    let corner_angle = |t: &crate::renderer::StaticAoLightmapTriangle, corner: usize| {
        let p = Vec3::from_array(t.positions[corner]);
        let u = (Vec3::from_array(t.positions[(corner + 1) % 3]) - p).normalize_or_zero();
        let v = (Vec3::from_array(t.positions[(corner + 2) % 3]) - p).normalize_or_zero();
        u.dot(v).clamp(-1.0, 1.0).acos()
    };
    let mut updates = Vec::new();
    for corners in welded.values() {
        if corners.len() < 2 { continue; }
        for &(ti, corner) in corners {
            let face = face_normals[ti];
            let mut sum = Vec3::ZERO;
            let mut members = 0;
            for &(other, other_corner) in corners {
                if face_normals[other].dot(face) < 0.707_106_77 { continue; }
                let n = Vec3::from_array(triangles[other].normals[other_corner]).normalize_or_zero();
                sum += n * corner_angle(&triangles[other], other_corner);
                members += 1;
            }
            let smooth = sum.normalize_or_zero();
            let original = Vec3::from_array(triangles[ti].normals[corner]).normalize_or_zero();
            if members >= 2 && smooth.is_finite() && smooth.length_squared() > 0.5
                && original.dot(smooth) < 0.999_99
            {
                updates.push((ti, corner, smooth));
            }
        }
    }
    for &(ti, corner, smooth) in &updates {
        triangles[ti].normals[corner] = smooth.to_array();
    }
    updates.len()
}

/// Hanika 2021 "Hacking the shadow terminator": move the ray origin from the
/// flat facet onto the curved surface implied by the shading normals. Without
/// this, a smoothed receiver on a coarse convex wall traces grazing sun rays
/// into its neighbouring facets and the self-shadow is faceted again.
fn terminator_origin(triangle: &crate::renderer::StaticAoLightmapTriangle, bary: [f32; 3]) -> Vec3 {
    let p = triangle.positions.map(Vec3::from_array);
    let position = p[0] * bary[0] + p[1] * bary[1] + p[2] * bary[2];
    let mut curved = Vec3::ZERO;
    for i in 0..3 {
        let n = Vec3::from_array(triangle.normals[i]).normalize_or_zero();
        curved += (position - n * (position - p[i]).dot(n).min(0.0)) * bary[i];
    }
    curved
}

/// A compiled BSP surface can reference a lightmap page even when its actual
/// authored shader never samples that page (e.g. an alpha-tested `rgbGen vertex`
/// seaweed patch). The draw stage, not the BSP's lightmap metadata or
/// LIGHTMAP_BY_VERTEX flag, determines whether the baked vertex RGB is visible.
/// Do not also tint a range that *does* consume a lightmap in another stage:
/// that would double-apply the cached sky gain to a lightmap-lit material.
/// The model-surface visibility switch still applies only to world models/*.
fn static_vertex_sky_receiver(
    is_model_material: bool,
    draw_model_surfaces: bool,
    rgb_gen: jka_assets::shader::RgbGen,
    class: DrawClass,
    additive_sky_stage: bool,
    has_lightmap_lighting: bool,
    world_or_compiled_vertex: bool,
) -> bool {
    (!is_model_material || draw_model_surfaces)
        && world_or_compiled_vertex
        && !has_lightmap_lighting
        && ((matches!(rgb_gen, jka_assets::shader::RgbGen::Vertex | jka_assets::shader::RgbGen::ExactVertex)
            && matches!(class, DrawClass::Opaque | DrawClass::Mask))
            || additive_sky_stage)
}

/// Valid bit 31, octahedral normal (2 x 10 bits), ambient sky visibility
/// (6 bits), and direct-light visibility (5 bits). Mode 1 stores both at 1;
/// mode 2 traces the two independent geometric visibility terms per luxel.
fn pack_sky_normal(normal: Vec3, ambient_open: f32, direct_open: f32) -> u32 {
    let n = normal / (normal.x.abs() + normal.y.abs() + normal.z.abs()).max(1e-6);
    let sign = |v: f32| if v >= 0.0 { 1.0 } else { -1.0 };
    let e = if n.z < 0.0 {
        [(1.0 - n.y.abs()) * sign(n.x), (1.0 - n.x.abs()) * sign(n.y)]
    } else {
        [n.x, n.y]
    };
    let q = |v: f32| ((v.clamp(-1.0, 1.0) * 0.5 + 0.5) * 1023.0).round() as u32;
    let ambient = (ambient_open.clamp(0.0, 1.0) * 63.0).round() as u32;
    let direct = (direct_open.clamp(0.0, 1.0) * 31.0).round() as u32;
    (1 << 31) | (q(e[1]) << 21) | (q(e[0]) << 11) | (direct << 6) | ambient
}

/// Barycentrics of a lightmap UV in a receiver triangle, or None outside it.
fn lightmap_barycentric(t: &crate::renderer::StaticAoLightmapTriangle, uv: [f32; 2]) -> Option<[f32; 3]> {
    let p = t.uvs;
    let area = (p[1][0]-p[0][0])*(p[2][1]-p[0][1]) - (p[1][1]-p[0][1])*(p[2][0]-p[0][0]);
    if area.abs() < 1e-12 { return None; }
    let q = [uv[0]-p[0][0], uv[1]-p[0][1]];
    let b = (q[0]*(p[2][1]-p[0][1])-q[1]*(p[2][0]-p[0][0]))/area;
    let c = ((p[1][0]-p[0][0])*q[1]-(p[1][1]-p[0][1])*q[0])/area;
    let a = 1.0-b-c;
    (a >= -0.00001 && b >= -0.00001 && c >= -0.00001).then_some([a, b, c])
}

/// Reconstruct shading normals only across *shared geometric edges* of the
/// same BSP material, with a 45-degree crease limit. Separate lightmap charts
/// may still meet on a continuous cylinder, but intersecting/overlaid surfaces
/// and deliberate 90-degree brush corners are not joined.
///
/// All of this happens once as map geometry is uploaded. The original vertices,
/// shadow-caster BVH and AO receivers remain exactly as authored.
fn smooth_sun_receiver_normals(
    triangles: &mut [crate::renderer::StaticAoLightmapTriangle],
) -> usize {
    // A fine position quantization tolerates BSP floating-point seam noise.
    // Never match vertices on normal/position alone: require a real common edge.
    type Position = [i32; 3];
    type Edge = (u32, Position, Position);
    fn point(p: [f32; 3]) -> Position {
        p.map(|v| (v * 32.0).round() as i32)
    }
    fn root(parent: &mut [usize], mut node: usize) -> usize {
        while parent[node] != node {
            let next = parent[node];
            parent[node] = parent[next];
            node = parent[node];
        }
        node
    }
    fn join(parent: &mut [usize], a: usize, b: usize) {
        let a = root(parent, a);
        let b = root(parent, b);
        if a != b { parent[b] = a; }
    }
    let count = triangles.len().saturating_mul(3);
    let mut parent: Vec<usize> = (0..count).collect();
    let mut edges: HashMap<Edge, Vec<(usize, usize, usize)>> = HashMap::new();
    let mut face_normals = Vec::with_capacity(triangles.len());
    for (ti, triangle) in triangles.iter().enumerate() {
        let n = triangle.normals.iter().fold(Vec3::ZERO, |sum, &n| {
            sum + Vec3::from_array(n)
        }).normalize_or_zero();
        face_normals.push(n);
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let pa = point(triangle.positions[a]);
            let pb = point(triangle.positions[b]);
            if pa == pb { continue; }
            let (first, last, ca, cb) = if pa < pb {
                (pa, pb, a, b)
            } else {
                (pb, pa, b, a)
            };
            edges.entry((triangle.material, first, last))
                .or_default().push((ti, ca, cb));
        }
    }
    let mut joined_edges = 0usize;
    for adjacent in edges.values() {
        // Non-manifold edges may represent overlapping surfaces or layers.
        if adjacent.len() != 2 { continue; }
        let (a, a0, a1) = adjacent[0];
        let (b, b0, b1) = adjacent[1];
        if a == b || face_normals[a].dot(face_normals[b]) < 0.707_106_77 {
            continue;
        }
        join(&mut parent, a * 3 + a0, b * 3 + b0);
        join(&mut parent, a * 3 + a1, b * 3 + b1);
        joined_edges += 1;
    }
    if joined_edges == 0 { return 0; }

    // Corner-angle weighting avoids triangulation bias: splitting a quad into
    // two triangles must not count as two independent curved-wall segments.
    let mut sums = vec![Vec3::ZERO; count];
    let mut weights = vec![0.0f32; count];
    let mut members = vec![0u32; count];
    for (ti, triangle) in triangles.iter().enumerate() {
        for corner in 0..3 {
            let p = Vec3::from_array(triangle.positions[corner]);
            let u = (Vec3::from_array(triangle.positions[(corner + 1) % 3]) - p)
                .normalize_or_zero();
            let v = (Vec3::from_array(triangle.positions[(corner + 2) % 3]) - p)
                .normalize_or_zero();
            let normal = Vec3::from_array(triangle.normals[corner]).normalize_or_zero();
            if normal.length_squared() < 0.5 || u.length_squared() < 0.5 || v.length_squared() < 0.5 {
                continue;
            }
            let weight = u.dot(v).clamp(-1.0, 1.0).acos();
            let group = root(&mut parent, ti * 3 + corner);
            sums[group] += normal * weight;
            weights[group] += weight;
            members[group] += 1;
        }
    }
    let mut changed = 0;
    for (ti, triangle) in triangles.iter_mut().enumerate() {
        for corner in 0..3 {
            let group = root(&mut parent, ti * 3 + corner);
            if members[group] < 2 || weights[group] <= f32::EPSILON { continue; }
            let smooth = sums[group].normalize_or_zero();
            let original = Vec3::from_array(triangle.normals[corner]).normalize_or_zero();
            if smooth.is_finite() && smooth.length_squared() > 0.5
                && original.dot(smooth) < 0.999_99
            {
                triangle.normals[corner] = smooth.to_array();
                changed += 1;
            }
        }
    }
    changed
}

#[derive(Clone, Debug, PartialEq)]
struct Key {
    direction: [f32; 3],
    cloud_coverage_steps: u8,
    visibility: SunVisibilityMode,
    models: bool,
    tinted: bool,
    sizes: Vec<[u32; 2]>,
    bake_sun: bool,
    aa: u8,
    /// r_bakedSunNormals receiver set, r_bakedSunTerminator origins, and the
    /// r_lightmapSignal 7/8 debug layers (replace flux/cosine with IDs).
    normals: u8,
    terminator: bool,
    debug_layers: u8,
    /// Baked sky world illumination: 0 off, 1 smoothed normals, 2 normals and
    /// sky visibility traced at the original lightmap resolution (`sky_sizes`).
    sky: u8,
    /// A/B sky normal algorithm: 0 smoothed (existing), 1 geometric face (live-pass parity).
    /// Only effective when sky > 0; toggling invalidates the static bake key.
    sky_ab: u8,
    sky_sizes: Vec<[u32; 2]>,
    local_sources: u32,
    local_sizes: Vec<[u32; 2]>,
    locals: Vec<(usize, scene::DynamicLight)>,
}

/// A q3map LIGHTMAP_BY_VERTEX BSP receiver without a lightmap atlas coordinate.
/// Its authored colour stays immutable; the sky-only GPU composition consumes
/// this position-independent baked visibility using the same packed format as luxels.
#[derive(Clone, Copy)]
pub(in crate::renderer) struct VertexSkySample {
    /// Direct draw-buffer address, including private inline BMODEL vertices.
    pub gpu_index: u32,
    /// Original compiled RGB for inline models (world RGB uses AO-adjusted source).
    pub authored_color: [f32; 4],
    pub packed: u32,
}

struct BakeOutput {
    sun: Vec<Vec<f32>>,
    local: Vec<Vec<f32>>,
    /// Per sun page: valid bit, octahedral normal, ambient and direct visibility.
    sky: Vec<Vec<u32>>,
    vertex_sky: Vec<VertexSkySample>,
}

/// Worker counts real completed receiver luxels.  The UI reserves 2% for the
/// render-thread upload/composition so 100% means the lightmap is actually ready.
#[derive(Default)]
struct BakeProgress {
    stage: AtomicU32,
    done: AtomicU32,
}
impl BakeProgress {
    fn set(&self, stage: u32, done: u32) {
        self.stage.store(stage, Ordering::Relaxed);
        self.done.fetch_max(done.min(980), Ordering::Relaxed);
    }
    fn fraction(&self, from: u32, span: u32, finished: u32, total: u32) {
        let done = from + (u64::from(span) * u64::from(finished.min(total))
            / u64::from(total.max(1))) as u32;
        self.done.fetch_max(done.min(980), Ordering::Relaxed);
    }
}

struct Pending {
    key: Key,
    progress: Arc<BakeProgress>,
    rx: mpsc::Receiver<Result<BakeOutput, String>>,
    cancel: Arc<AtomicBool>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub(in crate::renderer) struct StaticSun {
    pub geometry: Option<Geometry>,
    pending: Option<Pending>,
    key: Option<Key>,
    pub flux: Vec<GpuImage>,
    pub local_flux: Vec<GpuImage>,
    /// R32Uint packed smoothed normal + sky visibility per sun page.
    pub sky_normals: Vec<GpuImage>,
    pub vertex_sky: Vec<VertexSkySample>,
    pub local_lights: Vec<(usize, scene::DynamicLight)>,
    pub generation: u64,
    failed: Option<Key>,
    source_sizes: Vec<[u32; 2]>,
    bake_started: Option<Instant>,
    bake_workers: usize,
}

/// Upsample sunlight independently of the authored indirect light. Half-float
/// colour plus the R32F incident-flux image share a bounded 128 MiB budget.
pub(super) fn output_sizes(base: &[[u32; 2]], limit: u32) -> Vec<[u32; 2]> {
    if base.iter().any(|s| s[0] > limit || s[1] > limit)
        || base
            .iter()
            .map(|s| u64::from(s[0]) * u64::from(s[1]) * 16)
            .sum::<u64>()
            > 128 * 1024 * 1024
    {
        return Vec::new();
    }
    let mut scale = 4u32;
    while scale > 1
        && (base
            .iter()
            .any(|s| s[0].saturating_mul(scale) > limit || s[1].saturating_mul(scale) > limit)
            || base
                .iter()
                .map(|s| u64::from(s[0]) * u64::from(s[1]) * u64::from(scale * scale) * 16)
                .sum::<u64>()
                > 128 * 1024 * 1024)
    {
        scale /= 2;
    }
    base.iter().map(|s| [s[0] * scale, s[1] * scale]).collect()
}

pub(in crate::renderer) fn moving_static_material(
    batch: &DrawBatch,
    visibility: SunVisibilityMode,
) -> bool {
    (batch.alpha_cutoff != 0.0
        || (visibility == SunVisibilityMode::Filtered
            && batch.pipeline.class == DrawClass::Transparent
            && translucent_sun_caster(batch)))
        && !fixed_uv(batch)
}

/// Filtered sun visibility: a translucent surface occludes the sun only when
/// it is alpha blended over what lies behind it (dst * (1 - src alpha)), or is
/// an authored q3map2 alphaShadow / lightFilter caster. Additive glows and
/// clouds (GL_ONE GL_ONE) and modulate filters add or tint light; their
/// texture alpha is not coverage, so they never cast sun shadows.
pub(in crate::renderer) fn translucent_sun_caster(batch: &DrawBatch) -> bool {
    batch.alpha_shadow || batch.light_filter || matches!(
        batch.pipeline.blend,
        crate::scene::BlendMode::Custom(_, crate::materials::BlendFactor::OneMinusSrcAlpha)
    )
}

fn fixed_uv(batch: &DrawBatch) -> bool {
    batch.animation.is_none()
        && !matches!(batch.alpha_gen, jka_assets::shader::AlphaGen::Wave(_))
        && matches!(
            batch.tc_gen,
            jka_assets::shader::TcGen::Base
                | jka_assets::shader::TcGen::Lightmap
                | jka_assets::shader::TcGen::Vector(..)
        )
        && batch.tc_mods.iter().all(|m| {
            matches!(
                m,
                jka_assets::shader::TcMod::Scale(..) | jka_assets::shader::TcMod::Transform(..)
            )
        })
}

fn alpha_at(
    batch: &DrawBatch,
    vertices: &[GpuVertex; 3],
    bary: [f32; 3],
    image: &TextureData,
) -> f32 {
    let mut uv = [0.0; 2];
    let p = (0..3).fold(Vec3::ZERO, |p, i| {
        p + Vec3::from_array(vertices[i].position) * bary[i]
    });
    for axis in 0..2 {
        uv[axis] = (0..3)
            .map(|i| {
                (if matches!(batch.tc_gen, jka_assets::shader::TcGen::Lightmap) {
                    vertices[i].lightmap_uv[axis]
                } else {
                    vertices[i].uv[axis]
                }) * bary[i]
            })
            .sum();
    }
    if let jka_assets::shader::TcGen::Vector(s, t) = batch.tc_gen {
        uv = [p.dot(Vec3::from_array(s)), p.dot(Vec3::from_array(t))];
    }
    for m in batch.tc_mods.iter().take(4) {
        match *m {
            jka_assets::shader::TcMod::Scale(s, t) => {
                uv[0] *= s;
                uv[1] *= t;
            }
            jka_assets::shader::TcMod::Transform(v) => {
                uv = [
                    uv[0] * v[0] + uv[1] * v[2] + v[4],
                    uv[0] * v[1] + uv[1] * v[3] + v[5],
                ];
            }
            _ => {}
        }
    }
    let coord = |v: f32, n: u32| -> usize {
        ((if image.clamp {
            v.clamp(0.0, 1.0 - f32::EPSILON)
        } else {
            v - v.floor()
        }) * n as f32) as usize
    };
    let pixel = (coord(uv[1], image.height) * image.width as usize + coord(uv[0], image.width)) * 4;
    let vertex_alpha = if matches!(batch.alpha_gen, jka_assets::shader::AlphaGen::Vertex) {
        (0..3).map(|i| vertices[i].color[3] * bary[i]).sum()
    } else {
        1.0
    };
    image.rgba.get(pixel + 3).copied().unwrap_or(255) as f32 / 255.0 * batch.color[3] * vertex_alpha
}

fn fill_chart_border(values: &mut [f32], size: [u32; 2]) {
    let original = values.to_vec();
    for (i, value) in values.iter_mut().enumerate().filter(|(_, v)| **v < 0.0) {
        let x = i % size[0] as usize;
        let y = i / size[0] as usize;
        let mut sum = 0.0;
        let mut count = 0;
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
            if nx >= 0 && ny >= 0 && nx < size[0] as i32 && ny < size[1] as i32 {
                let v = original[ny as usize * size[0] as usize + nx as usize];
                if v >= 0.0 {
                    sum += v;
                    count += 1;
                }
            }
        }
        *value = if count > 0 { sum / count as f32 } else { 0.0 };
    }
}

/// Approximate the growing angular extent of light scattered by clouds.
/// Deterministic disk directions give actual penumbrae behind static casters;
/// blurring the composed lightmap would also blur texture/indirect detail.
/// The clear-sky path is exactly the historical single-ray path.
fn sun_sample_directions(toward: Vec3, coverage_steps: u8) -> Vec<Vec3> {
    if coverage_steps == 0 { return vec![toward]; }
    let coverage = f32::from(coverage_steps.min(20)) / 20.0;
    let count = if coverage < 0.35 { 5 } else if coverage < 0.70 { 9 } else { 13 };
    // ~23 degrees at full overcast: nearby roofs yield visibly broad edges.
    // We keep direct radiance separate, so the broad component is faint.
    let radius = (0.40 * coverage.powf(1.5)).tan();
    let tangent = toward.cross(if toward.y.abs() < 0.9 { Vec3::Y } else { Vec3::X }).normalize();
    let bitangent = toward.cross(tangent).normalize();
    let mut directions = Vec::with_capacity(count);
    directions.push(toward);
    for i in 1..count {
        let angle = i as f32 * 2.399_963_1; // golden angle
        let r = (((i as f32) - 0.5) / ((count - 1) as f32)).sqrt() * radius;
        directions.push((toward + r * (tangent * angle.cos() + bitangent * angle.sin())).normalize());
    }
    directions
}

/// Recover a receiver triangle per *covered* luxel. Built only for the
/// edge-AA A/B modes. Unlike an atlas-space blur, each lookup remains tied to
/// its original brush/batch chart, so different lightmap islands never bleed.
fn receiver_triangles(
    receivers: &[crate::renderer::StaticAoLightmapTriangle], page: usize, size: [u32; 2],
    samples: &[Option<StaticAoTexelSample>],
) -> Vec<usize> {
    let [w, h] = size;
    let mut ids = vec![usize::MAX; samples.len()];
    if w == 0 || h == 0 { return ids; }
    for (triangle_id, tri) in receivers.iter().enumerate() {
        if tri.texture != page { continue; }
        let p = tri.uvs.map(|uv| [uv[0] * w as f32 - 0.5, uv[1] * h as f32 - 0.5]);
        let edge = |a: [f32; 2], b: [f32; 2]| a[0] * b[1] - a[1] * b[0];
        let ab = [p[1][0]-p[0][0], p[1][1]-p[0][1]];
        let ac = [p[2][0]-p[0][0], p[2][1]-p[0][1]];
        let area = edge(ab, ac);
        if area.abs() < 1e-8 { continue; }
        let x0 = p.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min).floor().max(0.0) as u32;
        let y0 = p.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min).floor().max(0.0) as u32;
        let x1 = p.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max).ceil().min(w.saturating_sub(1) as f32) as u32;
        let y1 = p.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max).ceil().min(h.saturating_sub(1) as f32) as u32;
        for y in y0..=y1 { for x in x0..=x1 {
            let idx = y as usize * w as usize + x as usize;
            if ids[idx] != usize::MAX || samples[idx].is_none() { continue; }
            let q = [x as f32 - p[0][0], y as f32 - p[0][1]];
            let b = edge(q, ac) / area;
            let c = edge(ab, q) / area;
            if b >= -0.001 && c >= -0.001 && b + c <= 1.001 {
                ids[idx] = triangle_id;
            }
        }}
    }
    ids
}

/// Sample the real receiver triangle at a sub-luxel position. Search the
/// 3x3 adjacent center-owner IDs to handle a mesh's diagonal triangle seam.
/// Reject other chart owners; an atlas seam must never create a false penumbra.
fn subluxel_receiver(
    receivers: &[crate::renderer::StaticAoLightmapTriangle], ids: &[usize], size: [u32; 2],
    index: usize, dx: f32, dy: f32, terminator: bool,
) -> Option<StaticAoTexelSample> {
    let [w, h] = size;
    let x = (index % w as usize) as i32;
    let y = (index / w as usize) as i32;
    let central = receivers.get(ids[index])?;
    let uv = [(x as f32 + 0.5 + dx) / w as f32,
              (y as f32 + 0.5 + dy) / h as f32];
    let mut visited = [usize::MAX; 9];
    let mut used = 0;
    for (ox, oy) in [(0, 0),(-1,0),(1,0),(0,-1),(0,1),(-1,-1),(1,-1),(-1,1),(1,1)] {
        let nx = x + ox; let ny = y + oy;
        if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 { continue; }
        let id = ids[ny as usize * w as usize + nx as usize];
        if visited[..used].contains(&id) { continue; }
        visited[used] = id;
        used += 1;
        let Some(t) = receivers.get(id) else { continue; };
        if t.owner != central.owner || t.texture != central.texture { continue; }
        let Some([a, b, c]) = lightmap_barycentric(t, uv) else { continue; };
        let position = if terminator {
            terminator_origin(t, [a, b, c])
        } else {
            Vec3::from_array(t.positions[0]) * a
                + Vec3::from_array(t.positions[1]) * b
                + Vec3::from_array(t.positions[2]) * c
        };
        let normal = (Vec3::from_array(t.normals[0]) * a
                    + Vec3::from_array(t.normals[1]) * b
                    + Vec3::from_array(t.normals[2]) * c).normalize_or_zero();
        if normal.length_squared() < 0.5 { continue; }
        return Some(StaticAoTexelSample { position: position.to_array(), normal: normal.to_array() });
    }
    None
}

/// Only reconstruct discontinuities between luxels on a continuous receiving
/// surface. A shadow difference across unrelated atlased triangles is not an
/// edge to filter, even if the texels are adjacent in the lightmap texture.
fn continuous_neighbors(
    receivers: &[crate::renderer::StaticAoLightmapTriangle], ids: &[usize], samples: &[Option<StaticAoTexelSample>],
    a: usize, b: usize,
) -> bool {
    let Some(a_id) = receivers.get(ids[a]) else { return false; };
    let Some(b_id) = receivers.get(ids[b]) else { return false; };
    if a_id.owner != b_id.owner { return false; }
    let (Some(pa), Some(pb)) = (samples[a], samples[b]) else { return false; };
    let na = Vec3::from_array(pa.normal);
    let nb = Vec3::from_array(pb.normal);
    let delta = Vec3::from_array(pa.position) - Vec3::from_array(pb.position);
    na.dot(nb) > 0.975 && na.dot(delta).abs() < 0.75
}

fn sun_edge_indices(
    receivers: &[crate::renderer::StaticAoLightmapTriangle], ids: &[usize], samples: &[Option<StaticAoTexelSample>],
    flux: &[f32], size: [u32; 2], toward: Vec3,
) -> Vec<usize> {
    let w = size[0] as usize; let h = size[1] as usize;
    let mut out = Vec::new();
    for (i, sample) in samples.iter().enumerate() {
        let Some(sample) = sample else { continue; };
        let facing = Vec3::from_array(sample.normal).dot(toward).max(0.0);
        if facing < 0.03 || ids[i] == usize::MAX { continue; }
        let v = (flux[i]/facing).clamp(0.0, 1.0);
        let x = i%w; let y = i/w;
        let mut edge = false;
        for (dx,dy) in [(-1,0),(1,0),(0,-1),(0,1)] {
            let nx=x as i32+dx; let ny=y as i32+dy;
            if nx<0 || ny<0 || nx>=w as i32 || ny>=h as i32 { continue; }
            let ni=ny as usize*w+nx as usize;
            if !continuous_neighbors(receivers,ids,samples,i,ni) { continue; }
            let nf=Vec3::from_array(samples[ni].unwrap().normal).dot(toward).max(0.0);
            if nf>0.03 && (v-(flux[ni]/nf).clamp(0.0,1.0)).abs()>0.125 { edge=true; break; }
        }
        if edge { out.push(i); }
    }
    out
}

fn trace(geometry: &Geometry, key: &Key, cancel: &AtomicBool, progress: &BakeProgress) -> Result<BakeOutput, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    progress.set(1, 5);
    let mut triangles = Vec::new();
    let mut filters = Vec::new();
    let mut sky = Vec::new();
    for (index, batch) in geometry.batches.iter().enumerate() {
        if !key.models && geometry.model_materials[index] {
            continue;
        }
        if batch.pipeline.depth_equal {
            continue;
        }
        let is_sky = batch.pipeline.class == DrawClass::Sky
            && !matches!(batch.tc_gen, jka_assets::shader::TcGen::SkyCloud(_));
        let masked = batch.alpha_cutoff != 0.0;
        let translucent = key.visibility == SunVisibilityMode::Filtered
            && batch.pipeline.class == DrawClass::Transparent
            && translucent_sun_caster(batch);
        if !is_sky && batch.pipeline.class != DrawClass::Opaque && !masked && !translucent {
            continue;
        }
        if (masked || translucent) && !fixed_uv(batch) {
            continue;
        }
        if key.tinted
            && crate::renderer::lighting::glass_transmission::classify_glass(
                batch,
                batch
                    .texture
                    .and_then(|t| geometry.textures.get(t))
                    .map(|i| i.label.as_str()),
                None,
                true,
            )
            .is_some()
        {
            return Err("a glass caster requires live coloured transmission".into());
        }
        for triangle in geometry.vertices
            [batch.vertices.start as usize..batch.vertices.end as usize]
            .chunks_exact(3)
        {
            let v = [triangle[0], triangle[1], triangle[2]];
            let triangle = StaticAoRenderTriangle {
                positions: v.map(|v| v.position),
            };
            if is_sky {
                sky.push(triangle);
            } else {
                triangles.push(triangle);
                filters.push((masked || translucent).then_some((index, v, translucent)));
            }
        }
    }
    let solid = (!triangles.is_empty())
        .then(|| StaticAoBvh::build(Arc::new(triangles)))
        .transpose()?;
    let sky = (!sky.is_empty())
        .then(|| StaticAoBvh::build(Arc::new(sky)))
        .transpose()?;
    let visibility = |origin: Vec3, toward: Vec3, distance: f32| {
        let mut transmission = 1.0;
        let blocked = solid
            .as_ref()
            .and_then(|bvh| {
                bvh.trace_filtered(origin, toward, (distance - 0.1).max(0.0), |index, bary| {
                    let Some((batch, v, translucent)) = &filters[index] else {
                        return true;
                    };
                    let batch = &geometry.batches[*batch];
                    let alpha = batch
                        .texture
                        .and_then(|t| geometry.textures.get(t))
                        .map_or(1.0, |image| alpha_at(batch, v, bary, image));
                    if *translucent {
                        transmission *= 1.0 - alpha.clamp(0.0, 1.0);
                        transmission <= 0.001
                    } else if batch.alpha_cutoff > 0.0 {
                        alpha >= batch.alpha_cutoff
                    } else {
                        alpha < -batch.alpha_cutoff
                    }
                })
            })
            .is_some();
        if blocked { 0.0 } else { transmission }
    };
    progress.set(1, 25);
    let receivers = geometry.sun_receivers(key.normals);
    // B tests the normal the fullscreen shader sees on a flat BSP face.
    // Only sky shading/sky rays use it; sunlight flux and shadow-terminator
    // smoothing keep their original receiver normals in both A/B modes.
    let sky_face_receivers: Option<Vec<crate::renderer::StaticAoLightmapTriangle>> =
        (key.sky != 0 && key.sky_ab == 1).then(|| {
            geometry.receivers.iter().cloned().map(|mut tri| {
                let p = tri.positions.map(Vec3::from_array);
                let mut normal = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
                let authored = tri.normals.iter().fold(Vec3::ZERO, |a, &n| a + Vec3::from_array(n));
                if normal.dot(authored) < 0.0 { normal = -normal; }
                if normal.length_squared() < 0.5 {
                    normal = authored.normalize_or_zero();
                }
                tri.normals = [normal.to_array(); 3];
                tri
            }).collect()
        });
    let sky_receivers = sky_face_receivers.as_deref().unwrap_or(receivers);
    let mut samples = crate::renderer::static_ao::bvh::rasterize_lightmap_texels(
        receivers, &key.sizes, 1,
    );
    // Receiver triangle per covered luxel: terminator origins, edge AA and
    // the chart/normal debug layers. Owner-checked, so charts never mix.
    let page_ids: Vec<Vec<usize>> = if key.bake_sun && (key.terminator || key.aa != 0 || key.debug_layers != 0 || key.sky != 0) {
        samples.par_iter().zip(&key.sizes).enumerate()
            .map(|(page, (texels, &size))| receiver_triangles(receivers, page, size, texels))
            .collect()
    } else {
        Vec::new()
    };
    if key.bake_sun && key.debug_layers != 0 {
        progress.set(2, 40);
        let charts = triangle_charts(receivers);
        let hash = |id: u32| 0.05 + 0.95 * (id as f32 * 0.618_034).fract();
        let pages = samples.par_iter().zip(&page_ids).zip(&key.sizes)
            .map(|((texels, ids), &[w, _])| {
                let mut first = vec![0.0f32; texels.len()];
                let mut second = vec![0.0f32; texels.len()];
                for (i, sample) in texels.iter().enumerate() {
                    let (Some(sample), Some(t)) = (sample, receivers.get(ids[i])) else { continue; };
                    second[i] = hash(ids[i] as u32 + 1);
                    first[i] = if key.debug_layers == 7 {
                        hash(charts[ids[i]])
                    } else {
                        // Angle between the bake normal and the authored
                        // vertex normal at this luxel: what smoothing changed.
                        let uv = [((i % w as usize) as f32 + 0.5) / w as f32,
                                  ((i / w as usize) as f32 + 0.5) / (texels.len() / w as usize) as f32];
                        let raw = &geometry.receivers[ids[i]];
                        lightmap_barycentric(t, uv).map_or(0.0, |b| {
                            let n = (0..3).fold(Vec3::ZERO, |n, k| n + Vec3::from_array(raw.normals[k]) * b[k])
                                .normalize_or_zero();
                            let angle = n.dot(Vec3::from_array(sample.normal)).clamp(-1.0, 1.0).acos();
                            (angle / 20f32.to_radians()).clamp(0.0, 1.0)
                        })
                    };
                }
                first.extend(second);
                first
            })
            .collect();
        progress.set(6, 955);
        return Ok(BakeOutput { sun: pages, local: Vec::new(), sky: Vec::new(), vertex_sky: Vec::new() });
    }
    if key.bake_sun && key.terminator {
        samples.par_iter_mut().zip(&page_ids).zip(&key.sizes)
            .for_each(|((texels, ids), &[w, h])| {
                for (i, sample) in texels.iter_mut().enumerate() {
                    let (Some(sample), Some(t)) = (sample.as_mut(), receivers.get(ids[i])) else { continue; };
                    let uv = [((i % w as usize) as f32 + 0.5) / w as f32,
                              ((i / w as usize) as f32 + 0.5) / h as f32];
                    if let Some(b) = lightmap_barycentric(t, uv) {
                        sample.position = terminator_origin(t, b).to_array();
                    }
                }
            });
    }
    let toward = -Vec3::from_array(key.direction).normalize_or_zero();
    let sun_directions = sun_sample_directions(toward, key.cloud_coverage_steps);
    let sun_flux = |sample: StaticAoTexelSample| -> f32 {
        let normal = Vec3::from_array(sample.normal);
        let facing = normal.dot(toward).max(0.0);
        if facing == 0.0 { return 0.0; }
        let origin = Vec3::from_array(sample.position) + normal * 0.5;
        let mut transmission = 0.0;
        for &direction in &sun_directions {
            if cancel.load(Ordering::Relaxed) { break; }
            if normal.dot(direction) <= 0.0 { continue; }
            let distance = if key.visibility == SunVisibilityMode::Legacy {
                Some(131072.0)
            } else {
                sky.as_ref().and_then(|s| s.trace(origin, direction, 131072.0))
            };
            if let Some(distance) = distance {
                transmission += visibility(origin, direction, distance);
            }
        }
        facing * (transmission / sun_directions.len() as f32).clamp(0.0, 1.0)
    };
    progress.set(2, 40);
    let sun_total: u32 = samples.iter().map(|v| v.len() as u32).sum::<u32>().max(1);
    let sun_done = AtomicU32::new(0);
    let mut pages: Vec<Vec<f32>> = samples
        .par_iter()
        .zip(&key.sizes)
        .map(|(samples, size)| {
            let mut flux: Vec<f32> = samples
                .par_iter().enumerate()
                .map(|(i, sample)| {
                    if i % 128 == 0 {
                        let n = sun_done.fetch_add(128, Ordering::Relaxed) + 128;
                        progress.fraction(40, 560, n, sun_total);
                    }
                    if cancel.load(Ordering::Relaxed) { return -1.0; }
                    let Some(sample) = sample else { return -1.0; };
                    if !key.bake_sun { return 0.0; }
                    sun_flux(*sample)
                })
                .collect();
            let mut cosine: Vec<_> = samples.iter()
                .map(|s| s.map_or(-1.0, |s| Vec3::from_array(s.normal).dot(toward).max(0.0)))
                .collect();
            fill_chart_border(&mut flux, *size);
            fill_chart_border(&mut cosine, *size);
            flux.extend(cosine);
            flux
        })
        .collect();
    progress.set(2, 600);
    if cancel.load(Ordering::Relaxed) { return Err("cancelled".into()); }

    if key.bake_sun && key.aa != 0 {
        progress.set(3, 600);
        // Parallel by page; each update is gathered separately so no shared
        // GPU/CPU lightmap mutation or per-ray atomic overhead is introduced.
        let edge_pages: Vec<_> = samples.par_iter().zip(&pages).zip(&key.sizes).zip(&page_ids)
            .map(|(((receiver_samples, flux), &size), ids)| {
                (ids, sun_edge_indices(receivers, ids, receiver_samples, flux, size, toward))
            }).collect();
        let edge_total = edge_pages.iter().map(|(_, edges)| edges.len() as u32).sum::<u32>().max(1);
        let edge_done = AtomicU32::new(0);
        pages.par_iter_mut().zip(&samples).zip(&key.sizes).zip(&edge_pages)
            .for_each(|(((flux, receiver_samples), &size), (ids, edges))| {
                if cancel.load(Ordering::Relaxed) { return; }
                if key.aa == 1 {
                    // Small owner-aware visibility convolution only around true
                    // binary transitions. This changes neither ambient light nor
                    // the unoccluded cosine in the second flux layer.
                    let w = size[0] as usize; let h = size[1] as usize;
                    let updates: Vec<_> = edges.par_iter().enumerate().filter_map(|(ordinal, &i)| {
                        if ordinal % 32 == 0 {
                            let n=edge_done.fetch_add(32,Ordering::Relaxed)+32;
                            progress.fraction(600, 185, n, edge_total);
                        }
                        if cancel.load(Ordering::Relaxed) { return None; }
                        let sample=receiver_samples[i]?;
                        let facing=Vec3::from_array(sample.normal).dot(toward).max(0.0);
                        if facing <= 0.0 { return None; }
                        let x=i%w; let y=i/w;
                        let mut sum=0.0; let mut weight_sum=0.0;
                        for dy in -1i32..=1 { for dx in -1i32..=1 {
                            let nx=x as i32+dx; let ny=y as i32+dy;
                            if nx<0 || ny<0 || nx>=w as i32 || ny>=h as i32 { continue; }
                            let ni=ny as usize*w+nx as usize;
                            if !continuous_neighbors(receivers,ids,receiver_samples,i,ni) { continue; }
                            let ns=receiver_samples[ni]?;
                            let nf=Vec3::from_array(ns.normal).dot(toward).max(0.0);
                            if nf <= 0.03 { continue; }
                            let weight=(if dx==0 {2.0} else {1.0}) * (if dy==0 {2.0} else {1.0});
                            sum+=(flux[ni]/nf).clamp(0.0,1.0)*weight;
                            weight_sum+=weight;
                        }}
                        (weight_sum>0.0).then_some((i, facing*(sum/weight_sum).clamp(0.0,1.0)))
                    }).collect();
                    for (i, value) in updates { flux[i]=value; }
                } else {
                    let subgrid = if key.aa == 2 { 2 } else { 4 };
                    let updates: Vec<_> = edges.par_iter().enumerate().filter_map(|(ordinal, &i)| {
                        if ordinal % 32 == 0 {
                            let n=edge_done.fetch_add(32,Ordering::Relaxed)+32;
                            progress.fraction(600, 185, n, edge_total);
                        }
                        if cancel.load(Ordering::Relaxed) { return None; }
                        let sample=receiver_samples[i]?;
                        let facing=Vec3::from_array(sample.normal).dot(toward).max(0.0);
                        if facing <= 0.0 { return None; }
                        let mut sum=0.0;
                        for sy in 0..subgrid { for sx in 0..subgrid {
                            let dx=(sx as f32+0.5)/subgrid as f32-0.5;
                            let dy=(sy as f32+0.5)/subgrid as f32-0.5;
                            let hit=subluxel_receiver(receivers,ids,size,i,dx,dy,key.terminator).unwrap_or(sample);
                            let subf=Vec3::from_array(hit.normal).dot(toward).max(0.0);
                            sum+=if subf>0.0 { sun_flux(hit)/subf } else { 0.0 };
                        }}
                        Some((i,facing*(sum/(subgrid*subgrid) as f32).clamp(0.0,1.0)))
                    }).collect();
                    for (i,value) in updates { flux[i]=value; }
                }
            });
        progress.set(3, 785);
    }
    // Use the shader stages actually consuming the original vertex RGB,
    // including alpha-tested textures/* foliage with a BSP lightmap reference
    // that none of their stages use. Keep explicit/implicit lightmap stages
    // authoritative: a vertex-colored sibling must not receive another sky
    // multiplier over an already composed lightmap. Stages share a geometry
    // range, so classify that range once before tracing any vertex.
    let vertex_sky: Vec<VertexSkySample> = if key.bake_sun && key.sky == 2 {
        let lightmap_lit_ranges: std::collections::BTreeSet<(u32, u32)> = geometry.batches.iter()
            .flat_map(|batch| std::iter::once(batch).chain(batch.additive_companions.iter()))
            .filter(|batch| batch.modulate_lightmap || batch.texture_is_lightmap || batch.lightstyle_slot != 0)
            .map(|batch| (batch.vertices.start, batch.vertices.end))
            .collect();
        let mut positions = std::collections::BTreeMap::<usize, Vec3>::new();
        for (batch_index, batch) in geometry.batches.iter().enumerate() {
            let range = (batch.vertices.start, batch.vertices.end);
            let lightmap_lit = batch.lightstyle_slot != 0 || lightmap_lit_ranges.contains(&range);
            // World stage lists include all siblings, so an unused BSP page
            // must not veto vertex RGB. Inline models currently retain only
            // selected bake stages; keep their old conservative eligibility
            // until their complete sibling stage lists are available here.
            let world_or_compiled_vertex = batch_index < geometry.world_batch_count
                || (batch.vertex_lit && batch.lightmap.is_none());
            if !static_vertex_sky_receiver(
                geometry.model_materials[batch_index], key.models,
                batch.rgb_gen, batch.pipeline.class,
                batch.baked_sky_additive_stage() && batch.additive_companions.iter()
                    .all(crate::scene::DrawBatch::baked_sky_additive_stage),
                lightmap_lit, world_or_compiled_vertex,
            ) {
                continue;
            }
            for (tri_index, triangle) in geometry.vertices
                [batch.vertices.start as usize..batch.vertices.end as usize]
                .chunks_exact(3).enumerate()
            {
                let p = [
                    Vec3::from_array(triangle[0].position),
                    Vec3::from_array(triangle[1].position),
                    Vec3::from_array(triangle[2].position),
                ];
                let mut normal = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
                let authored = triangle.iter().fold(Vec3::ZERO, |sum, v| sum + Vec3::from_array(v.normal));
                if normal.dot(authored) < 0.0 { normal = -normal; }
                if normal.length_squared() < 0.5 { normal = authored.normalize_or_zero(); }
                for corner in 0..3 {
                    let index = batch.vertices.start as usize + tri_index * 3 + corner;
                    positions.entry(index).or_insert(normal);
                }
            }
        }
        positions.into_par_iter().filter_map(|(source_index, normal)| {
            let gpu_index = *geometry.vertex_gpu_indices.get(source_index)?;
            if gpu_index == u32::MAX { return None; }
            let vertex = &geometry.vertices[source_index];
            let position = Vec3::from_array(vertex.position);
            let normal = normal.normalize_or_zero();
            if normal.length_squared() < 0.5 || cancel.load(Ordering::Relaxed) {
                return None;
            }
            let tangent = normal.cross(if normal.y.abs() < 0.9 { Vec3::Y } else { Vec3::X }).normalize();
            let bitangent = normal.cross(tangent);
            let origin = position + normal * 0.5;
            let mut open = 0.0;
            const RAYS: usize = 16;
            for i in 0..RAYS {
                let u = (i as f32 + 0.5) / RAYS as f32;
                let phi = i as f32 * 2.399_963_1;
                let radius = u.sqrt();
                let direction = (tangent * (radius * phi.cos()) + bitangent * (radius * phi.sin())
                    + normal * (1.0 - u).sqrt()).normalize();
                let distance = if key.visibility == SunVisibilityMode::Legacy {
                    Some(131072.0)
                } else { sky.as_ref().and_then(|s| s.trace(origin, direction, 131072.0)) };
                if let Some(distance) = distance { open += visibility(origin, direction, distance); }
            }
            let facing = normal.dot(toward).max(0.0);
            let direct = if facing > 0.0 {
                (sun_flux(StaticAoTexelSample {
                    position: position.to_array(), normal: normal.to_array(),
                }) / facing).clamp(0.0, 1.0)
            } else { 0.0 };
            Some(VertexSkySample { gpu_index, authored_color: vertex.color, packed: pack_sky_normal(normal, open / RAYS as f32, direct) })
        }).collect()
    } else { Vec::new() };
    if cancel.load(Ordering::Relaxed) { return Err("cancelled".into()); }

    let sky_pages = if key.bake_sun && key.sky != 0 {
        // The previous 87% "EDGE AA" state hid all sky tracing time. Reserve
        // a measured interval for the sky's 16 ambient hemisphere rays.
        // Direct visibility reuses the already completed sun/moon bake.
        progress.set(4, 785);
        let charts = triangle_charts(receivers);
        // Trace at ORIGINAL resolution, then resample within the same chart.
        // Only diffuse sky openness belongs on this lower-resolution grid.
        // The compositor reads direct visibility from the antialiased sun flux.
        let base_visibility: Vec<Vec<f32>> = if key.sky == 2 {
            let base_samples = crate::renderer::static_ao::bvh::rasterize_lightmap_texels(
                sky_receivers, &key.sky_sizes, 1,
            );
            const RAYS: usize = 16;
            let sky_total = base_samples.iter().map(|texels| texels.len() as u32).sum::<u32>().max(1);
            let sky_done = AtomicU32::new(0);
            base_samples.par_iter().map(|texels| texels.par_iter().enumerate().map(|(ordinal, sample)| {
                if ordinal % 64 == 0 {
                    let n = sky_done.fetch_add(64, Ordering::Relaxed) + 64;
                    progress.fraction(785, 100, n, sky_total);
                }
                let Some(sample) = sample else { return -1.0; };
                if cancel.load(Ordering::Relaxed) { return -1.0; }
                let normal = Vec3::from_array(sample.normal);
                let tangent = normal.cross(if normal.y.abs() < 0.9 { Vec3::Y } else { Vec3::X }).normalize();
                let bitangent = normal.cross(tangent);
                let origin = Vec3::from_array(sample.position) + normal * 0.5;
                let mut open = 0.0;
                for i in 0..RAYS {
                    // Cosine-weighted golden-angle hemisphere: irradiance weighting.
                    let u = (i as f32 + 0.5) / RAYS as f32;
                    let phi = i as f32 * 2.399_963_1;
                    let radius = u.sqrt();
                    let direction = (tangent * (radius * phi.cos()) + bitangent * (radius * phi.sin())
                        + normal * (1.0 - u).sqrt()).normalize();
                    let distance = if key.visibility == SunVisibilityMode::Legacy {
                        Some(131072.0)
                    } else {
                        sky.as_ref().and_then(|s| s.trace(origin, direction, 131072.0))
                    };
                    if let Some(distance) = distance {
                        open += visibility(origin, direction, distance);
                    }
                }
                open / RAYS as f32
            }).collect()).collect()
        } else {
            Vec::new()
        };
        progress.set(4, 885);
        let base_charts: Vec<Vec<u32>> = if key.sky == 2 {
            let mut charted = receivers.to_vec();
            for (triangle, &chart) in charted.iter_mut().zip(&charts) { triangle.owner = chart; }
            crate::renderer::static_ao::bvh::rasterize_lightmap_owners(&charted, &key.sky_sizes, 1)
        } else {
            Vec::new()
        };
        // A uses the static sun's smoothed luxel normals. B rasterizes
        // geometric face normals into the same luxels, without changing sun
        // flux or allocating any additional GPU textures.
        let face_samples = sky_face_receivers.as_ref().map(|face| {
            crate::renderer::static_ao::bvh::rasterize_lightmap_texels(face, &key.sizes, 1)
        });
        let pack_done = AtomicU32::new(0);
        let pack_total = samples.len() as u32;
        samples.par_iter().zip(&page_ids).zip(&key.sizes).enumerate()
            .map(|(page, ((texels, ids), &[w, h]))| {
                let mut packed: Vec<u32> = texels.iter().enumerate().map(|(i, sample)| {
                    let Some(sample) = sample else { return 0; };
                    let exposure = if key.sky == 2 {
                        let chart = receivers.get(ids[i]).map_or(0, |_| charts[ids[i]]);
                        let [bw, bh] = key.sky_sizes[page];
                        let at = [((i % w as usize) as f32 + 0.5) * bw as f32 / w as f32 - 0.5,
                                  ((i / w as usize) as f32 + 0.5) * bh as f32 / h as f32 - 0.5];
                        let (x0, y0) = (at[0].floor(), at[1].floor());
                        let (fx, fy) = (at[0] - x0, at[1] - y0);
                        let mut sum = 0.0;
                        let mut weights = 0.0;
                        for (dx, dy, weight) in [(0, 0, (1.0 - fx) * (1.0 - fy)), (1, 0, fx * (1.0 - fy)),
                                                 (0, 1, (1.0 - fx) * fy), (1, 1, fx * fy)] {
                            let (x, y) = (x0 as i32 + dx, y0 as i32 + dy);
                            if x < 0 || y < 0 || x >= bw as i32 || y >= bh as i32 { continue; }
                            let index = y as usize * bw as usize + x as usize;
                            let value = base_visibility[page][index];
                            if value < 0.0 || base_charts[page][index] != chart { continue; }
                            sum += value * weight;
                            weights += weight;
                        }
                        if weights > 1e-4 { sum / weights } else { 1.0 }
                    } else {
                        1.0
                    };
                    let normal = face_samples.as_ref()
                        .and_then(|pages| pages.get(page))
                        .and_then(|page_samples| page_samples.get(i))
                        .and_then(|sample| sample.as_ref())
                        .map_or(Vec3::from_array(sample.normal), |face| Vec3::from_array(face.normal));
                    // Luxel direct visibility stays in the existing R32F sun
                    // texture; these packed bits are only used by vertex sky.
                    pack_sky_normal(normal, exposure, 0.0)
                }).collect();
                // Border luxels take a covered neighbour so bilinear world
                // sampling at chart edges never sees an ungained texel.
                let original = packed.clone();
                for (i, value) in packed.iter_mut().enumerate().filter(|(_, v)| **v == 0) {
                    let (x, y) = ((i % w as usize) as i32, (i / w as usize) as i32);
                    *value = [(-1, 0), (1, 0), (0, -1), (0, 1)].into_iter()
                        .filter(|(dx, dy)| x + dx >= 0 && y + dy >= 0 && x + dx < w as i32 && y + dy < h as i32)
                        .map(|(dx, dy)| original[(y + dy) as usize * w as usize + (x + dx) as usize])
                        .find(|v| *v != 0)
                        .unwrap_or(0);
                }
                let completed = pack_done.fetch_add(1, Ordering::Relaxed) + 1;
                progress.fraction(885, 30, completed, pack_total);
                packed
            })
            .collect()
    } else {
        Vec::new()
    };
    progress.set(5, 915);
    let local_samples = crate::renderer::static_ao::bvh::rasterize_lightmap_texels(
        &geometry.receivers,
        &key.local_sizes,
        1,
    );
    let local_done=AtomicU32::new(0);
    let local_total=key.local_sizes.len() as u32;
    let local = local_samples
        .par_iter()
        .zip(&key.local_sizes)
        .map(|(samples, size)| {
            let mut page = Vec::with_capacity(samples.len() * (key.locals.len() + 3));
            let mut totals = vec![[0.0f32; 3]; samples.len()];
            for (_, light) in &key.locals {
                let mut layer: Vec<f32> = samples
                    .par_iter()
                    .map(|sample| {
                        if cancel.load(Ordering::Relaxed) {
                            return -1.0;
                        }
                        let Some(sample) = sample else {
                            return -1.0;
                        };
                        let position = Vec3::from_array(sample.position);
                        let normal = Vec3::from_array(sample.normal);
                        let incident = super::lightmap_local::irradiance(light, position, normal);
                        if incident <= 0.0 {
                            return 0.0;
                        }
                        let origin = position + normal * 0.5;
                        let delta = Vec3::from_array(light.position) - origin;
                        incident * visibility(origin, delta.normalize_or_zero(), delta.length())
                    })
                    .collect();
                fill_chart_border(&mut layer, *size);
                for (total, value) in totals.iter_mut().zip(&layer) {
                    for c in 0..3 { total[c] += value * light.color[c]; }
                }
                page.extend(layer);
            }
            for c in 0..3 { page.extend(totals.iter().map(|total| total[c])); }
            let n=local_done.fetch_add(1,Ordering::Relaxed)+1;
            progress.fraction(915, 35, n, local_total);
            page
        })
        .collect();
    progress.set(6, 955);
    if cancel.load(Ordering::Relaxed) {
        Err("cancelled".into())
    } else {
        Ok(BakeOutput { sun: pages, local, sky: sky_pages, vertex_sky })
    }
}

impl Renderer {
    /// Common frame-boundary entry: commands only change inputs. Old results
    /// cannot be used after a sun/material change, even while a worker finishes.
    pub(in crate::renderer) fn sync_static_sun_lightmaps(&mut self) {
        // The cached static illumination needs flux/occlusion from the same
        // direction as the runtime CSM. Only its DIRECTION is part of the key;
        // moving the intensity slider must not re-raytrace static geometry.
        let sun = if self.sun_override {
            self.relit_celestial()
        } else {
            self.active_sun()
        };
        let bake_sun = self.baked_sun_cache
            && self.baked_sun_debug_mode == 0
            && self.cascaded_shadow_mode.is_csm();
        let cloud_coverage_steps = if bake_sun { self.relight_cloud_coverage_steps() } else { 0 };
        let light_sources = self.added_map_light_sources();
        let bake_local = light_sources != 0 && self.baked_sun_debug_mode == 0;
        let debug_layers = if matches!(self.lightmap_signal, 7 | 8) { self.lightmap_signal } else { 0 };
        // B uses geometric normals and 16-ray ambient sky openness. Direct
        // visibility comes from the existing sun/moon bake and its edge AA.
        let sky = if bake_sun && self.sun_override && self.sky_world_bake_ab != 0
            && self.sky_world_lighting_strength > 0.0 {
            2
        } else {
            0
        };
        let Some(world) = self.world.as_mut() else {
            return;
        };
        let state = &mut world.static_sun;
        if self.sun_bake_deferred {
            // A drag can begin while a previous trace is still running.
            // Dropping Pending cancels it; no new worker starts until release.
            if state.pending.take().is_some() {
                state.bake_started = None;
            }
            return;
        }
        if (!bake_sun && !bake_local) || world.source_map || state.geometry.is_none() {
            state.pending = None;
            state.key = None;
            state.bake_started = None;
            state.flux.clear();
            state.local_flux.clear();
            state.sky_normals.clear();
            state.vertex_sky.clear();
            state.local_lights.clear();
            return;
        }
        let images = world
            .baked_sun_cache
            .as_ref()
            .map_or(&world.lightmaps, |c| &c.original);
        let same_sizes =
            state.source_sizes.len() == images.len()
                && state.source_sizes.iter().zip(images).all(|(size, image)| {
                    *size == [image._texture.width(), image._texture.height()]
                });
        let requested = state
            .key
            .as_ref()
            .or_else(|| state.pending.as_ref().map(|p| &p.key))
            .or(state.failed.as_ref());
        let same_request = same_sizes
            && requested.is_some_and(|k| {
                k.direction == sun.direction
                    && k.cloud_coverage_steps == cloud_coverage_steps
                    && k.bake_sun == bake_sun
                    && k.aa == self.baked_sun_aa
                    && k.normals == self.sun_receiver_normals
                    && k.terminator == self.sun_terminator
                    && k.debug_layers == debug_layers
                    && k.sky == sky
                    && k.sky_ab == (if sky != 0 { 1 } else { 0 })
                    && k.local_sources
                        == if bake_local {
                            light_sources
                        } else {
                            0
                        }
                    && k.visibility == self.sun_visibility
                    && k.models == self.draw_bsp_model_surfaces
                    && k.tinted == self.tinted_glass_enabled
            });
        let outcome = if same_request {
            state.pending.as_ref().and_then(|p| match p.rx.try_recv() {
                Ok(value) => Some(value),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("static sunlight worker disconnected".into()))
                }
            })
        } else {
            None
        };
        if same_request && outcome.is_none() {
            return;
        }
        let locals: Vec<_> = if bake_local {
            world
                .static_bake_sources
                .iter()
                .copied()
                .filter(|(_, light)| super::lightmap_local::source_enabled(light, light_sources))
                .take(super::lightmap_local::MAX_SOURCES.min(self.device.limits().max_texture_array_layers.saturating_sub(3) as usize))
                .collect()
        } else {
            Vec::new()
        };
        let base: Vec<_> = images
            .iter()
            .map(|i| [i._texture.width(), i._texture.height()])
            .collect();
        state.source_sizes = base.clone();
        let key = Key {
            direction: sun.direction,
            cloud_coverage_steps,
            visibility: self.sun_visibility,
            models: self.draw_bsp_model_surfaces,
            tinted: self.tinted_glass_enabled,
            sizes: if bake_sun {
                output_sizes(&base, self.device.limits().max_texture_dimension_2d / 2)
            } else {
                base.clone()
            },
            bake_sun,
            aa: self.baked_sun_aa,
            normals: self.sun_receiver_normals,
            terminator: self.sun_terminator,
            debug_layers,
            sky,
            sky_ab: if sky != 0 { 1 } else { 0 },
            sky_sizes: if sky == 2 { base.clone() } else { Vec::new() },
            local_sources: if bake_local {
                light_sources
            } else {
                0
            },
            local_sizes: super::lightmap_local::output_sizes(
                &base,
                locals.len(),
                self.device.limits().max_texture_dimension_2d,
            ),
            locals,
        };
        if key.sizes.is_empty() {
            if state.failed.as_ref() != Some(&key) {
                println!(
                    "[SUN LIGHTMAP] output exceeds the lightmap budget or device limit; retaining live CSM"
                );
            }
            state.failed = Some(key);
            state.pending = None;
            state.key = None;
            state.bake_started = None;
            state.flux.clear();
            return;
        }
        if state.key.as_ref() == Some(&key) || state.failed.as_ref() == Some(&key) {
            return;
        }
        if state.pending.as_ref().is_some_and(|p| p.key != key) {
            state.pending = None;
        }
        state.key = None;
        state.local_lights.clear();
        if let Some(outcome) = outcome {
            state.pending = None;
            match outcome {
                Ok(pages) => {
                    state.flux = pages
                        .sun
                        .iter()
                        .zip(&key.sizes)
                        .map(|(pixels, size)| {
                            use wgpu::util::DeviceExt;
                            let texture = self.device.create_texture_with_data(
                                &self.queue,
                                &wgpu::TextureDescriptor {
                                    label: Some("static sunlight incident flux"),
                                    size: wgpu::Extent3d {
                                        width: size[0],
                                        height: size[1] * 2,
                                        depth_or_array_layers: 1,
                                    },
                                    mip_level_count: 1,
                                    sample_count: 1,
                                    dimension: wgpu::TextureDimension::D2,
                                    format: wgpu::TextureFormat::R32Float,
                                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                                        | wgpu::TextureUsages::COPY_DST,
                                    view_formats: &[],
                                },
                                wgpu::util::TextureDataOrder::LayerMajor,
                                bytemuck::cast_slice(pixels),
                            );
                            let view = texture.create_view(&Default::default());
                            GpuImage::new(texture, view)
                        })
                        .collect();
                    state.local_flux = pages
                        .local
                        .iter()
                        .zip(&key.local_sizes)
                        .map(|(pixels, size)| {
                            use wgpu::util::DeviceExt;
                            let texture = self.device.create_texture_with_data(
                                &self.queue,
                                &wgpu::TextureDescriptor {
                                    label: Some("baked per-light local irradiance"),
                                    size: wgpu::Extent3d {
                                        width: size[0],
                                        height: size[1],
                                        depth_or_array_layers: (key.locals.len() + 3) as u32,
                                    },
                                    mip_level_count: 1,
                                    sample_count: 1,
                                    dimension: wgpu::TextureDimension::D2,
                                    format: wgpu::TextureFormat::R32Float,
                                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                                        | wgpu::TextureUsages::COPY_DST,
                                    view_formats: &[],
                                },
                                wgpu::util::TextureDataOrder::LayerMajor,
                                bytemuck::cast_slice(pixels),
                            );
                            let view = texture.create_view(&wgpu::TextureViewDescriptor {
                                dimension: Some(wgpu::TextureViewDimension::D2Array),
                                ..Default::default()
                            });
                            GpuImage::new(texture, view)
                        })
                        .collect();
                    state.sky_normals = pages.sky.iter().zip(&key.sizes).map(|(packed, size)| {
                        use wgpu::util::DeviceExt;
                        let texture = self.device.create_texture_with_data(
                            &self.queue,
                            &wgpu::TextureDescriptor {
                                label: Some("static sun bake normals and sky visibility"),
                                size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                                mip_level_count: 1,
                                sample_count: 1,
                                dimension: wgpu::TextureDimension::D2,
                                format: wgpu::TextureFormat::R32Uint,
                                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                                view_formats: &[],
                            },
                            wgpu::util::TextureDataOrder::LayerMajor,
                            bytemuck::cast_slice(packed),
                        );
                        let view = texture.create_view(&Default::default());
                        GpuImage::new(texture, view)
                    }).collect();
                    state.vertex_sky = pages.vertex_sky;
                    state.local_lights = key.locals.clone();
                    if key.sky == 2 {
                        println!("[SUN LIGHTMAP] B static BSP vertex-lit sky receivers: {}", state.vertex_sky.len());
                    }
                    if let Some(started) = state.bake_started {
                        println!(
                            "[SUN LIGHTMAP] tracing + texture upload: {:.2}s ({} workers, AA {})",
                            started.elapsed().as_secs_f64(), state.bake_workers, key.aa
                        );
                    }
                    println!(
                        "[LOCAL LIGHTMAP] baked {} static emitters into {} page(s), sizes {:?}",
                        key.locals.len(),
                        state.local_flux.len(),
                        key.local_sizes
                    );
                    state.key = Some(key);
                    state.generation = state.generation.wrapping_add(1).max(1);
                    println!(
                        "[SUN LIGHTMAP] static visibility ready, {} page(s), sizes {:?}, relit cloud coverage {}% ({} sun rays/texel); camera movement does not rebake",
                        state.flux.len(),
                        state
                            .flux
                            .iter()
                            .map(|i| [i._texture.width(), i._texture.height() / 2])
                            .collect::<Vec<_>>(),
                        state.key.as_ref().unwrap().cloud_coverage_steps as u32 * 5,
                        sun_sample_directions(-Vec3::from_array(state.key.as_ref().unwrap().direction).normalize_or_zero(), state.key.as_ref().unwrap().cloud_coverage_steps).len(),
                    );
                }
                Err(e) => {
                    let elapsed = state.bake_started.take().map_or(0.0, |start| start.elapsed().as_secs_f64());
                    eprintln!("[SUN LIGHTMAP] bake failed after {elapsed:.2}s: {e}");
                    state.failed = Some(key);
                }
            }
            return;
        }
        if state.pending.is_none() {
            let geometry = state.geometry.clone().unwrap();
            let worker_key = key.clone();
            let cancel = Arc::new(AtomicBool::new(false));
            let worker_cancel = Arc::clone(&cancel);
            let progress = Arc::new(BakeProgress::default());
            let worker_progress = Arc::clone(&progress);
            let (tx, rx) = mpsc::channel();
            let workers = bake_worker_count(self.bake_threads, 1);
            let started = Instant::now();
            match std::thread::Builder::new()
                .name("static-sun-lightmap".into())
                .spawn(move || {
                    let pool = rayon::ThreadPoolBuilder::new()
                        .num_threads(workers)
                        .build();
                    let result = pool.map_err(|e| e.to_string()).and_then(|pool| {
                        pool.install(|| trace(&geometry, &worker_key, &worker_cancel, &worker_progress))
                    });
                    let _ = tx.send(result);
                }) {
                Ok(_) => {
                    state.bake_started = Some(started);
                    state.bake_workers = workers;
                    println!("[SUN LIGHTMAP] bake started: {} workers, AA {}, normals {}, terminator {}, debug layers {}, sky {}", workers, key.aa, key.normals, u8::from(key.terminator), key.debug_layers, key.sky);
                    state.pending = Some(Pending { key, rx, cancel, progress });
                }
                Err(e) => {
                    state.bake_started = None;
                    eprintln!("[SUN LIGHTMAP] could not start worker: {e}");
                    state.failed = Some(key);
                }
            }
        }
    }
}

impl Renderer {
    /// Poll only: never starts a bake or blocks on worker/GPU completion.
    /// Called after each render so Ready means the GPU lightmap was composed.
    pub(in crate::renderer) fn static_sun_bake_status(&mut self) -> (u32, SunBakeStage) {
        if self.sun_bake_deferred { return (0, SunBakeStage::Off); }
        let Some(world) = self.world.as_mut() else { return (0, SunBakeStage::Off); };
        let state = &mut world.static_sun;
        if let Some(pending) = state.pending.as_ref() {
            let stage = match pending.progress.stage.load(Ordering::Relaxed) {
                2 => SunBakeStage::Tracing,
                3 => SunBakeStage::Refining,
                4 => SunBakeStage::SkyVisibility,
                5 => SunBakeStage::Local,
                6 => SunBakeStage::Uploading,
                _ => SunBakeStage::Preparing,
            };
            return (pending.progress.done.load(Ordering::Relaxed).min(980), stage);
        }
        if state.key.is_some() {
            let composed = world.baked_sun_cache.as_ref().is_some_and(|c| c.active && c.has_composition());
            if composed {
                if let Some(started) = state.bake_started.take() {
                    println!(
                        "[SUN LIGHTMAP] BAKE READY in {:.2}s total (AA {}, {} workers, {} sun pages, {} local pages); GPU composition submitted",
                        started.elapsed().as_secs_f64(),
                        state.key.as_ref().map_or(0, |k| k.aa),
                        state.bake_workers,
                        state.flux.len(),
                        state.local_flux.len()
                    );
                }
                return (1000, SunBakeStage::Ready);
            }
            return (1000, SunBakeStage::Fallback);
        }
        if state.failed.is_some() { return (0, SunBakeStage::Failed); }
        (0, SunBakeStage::Off)
    }
}

impl StaticSun {
    pub(in crate::renderer) fn new(geometry: Option<Geometry>) -> Self {
        Self {
            geometry,
            ..Default::default()
        }
    }
    pub(in crate::renderer) fn replace_geometry(&mut self, geometry: Geometry) {
        *self = Self::new(Some(geometry));
    }
    pub(in crate::renderer) fn local_ready(&self) -> bool {
        self.key.is_some() && !self.local_flux.is_empty() && !self.local_lights.is_empty()
    }
    pub(in crate::renderer) fn ready(&self) -> bool {
        self.key.as_ref().is_some_and(|k| k.bake_sun) && !self.flux.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_sky_bake_uses_world_stage_lighting_not_bsp_lightmap_metadata() {
        use jka_assets::shader::RgbGen;
        let eligible = |model, draw_model, lightmap_lit, world_or_vertex, rgb, class| {
            static_vertex_sky_receiver(model, draw_model, rgb, class,
                false, lightmap_lit, world_or_vertex)
        };
        // seph_seaweed3: BSP references lm_0026, but the masked base is
        // rgbGen vertex and no stage samples the lightmap. Its world receiver
        // is eligible even without an original LIGHTMAP_BY_VERTEX flag.
        assert!(eligible(false, true, false, true, RgbGen::Vertex, DrawClass::Mask));
        assert!(eligible(false, false, false, true, RgbGen::Vertex, DrawClass::Mask));
        // The compiled models/* receiver remains supported, but honors the
        // built-in world model visibility switch.
        assert!(eligible(true, true, false, true, RgbGen::ExactVertex, DrawClass::Opaque));
        assert!(!eligible(true, false, false, true, RgbGen::Vertex, DrawClass::Mask));
        // Do not multiply a stage already lit by an implicit or explicit
        // lightmap (including a lightmap-consuming sibling in that range).
        assert!(!eligible(false, true, true, true, RgbGen::Vertex, DrawClass::Opaque));
        assert!(!eligible(false, true, false, true, RgbGen::Identity, DrawClass::Opaque));
        assert!(!eligible(false, true, false, true, RgbGen::Vertex, DrawClass::Transparent));
        // Constant/wave additive stages can also receive a cached sky gain:
        // their authored RGB must remain unchanged, and the shader applies
        // this gain *after* rgbGen wave evaluation. Alpha is never modified.
        assert!(static_vertex_sky_receiver(false, true, RgbGen::Const,
            DrawClass::Transparent, true, false, true));
        assert!(static_vertex_sky_receiver(false, true, RgbGen::Wave(jka_assets::shader::Wave {
            func: jka_assets::shader::WaveFunc::Sin, base: 1.0,
            amplitude: 0.0, phase: 0.0, frequency: 0.0,
        }), DrawClass::Transparent, true, false, true));
        assert!(!static_vertex_sky_receiver(false, true, RgbGen::Const,
            DrawClass::Transparent, true, true, true));
        // Inline models with no compiled vertex lighting keep their previous
        // conservative behavior until complete material stages are retained.
        assert!(!eligible(false, true, false, false, RgbGen::Vertex, DrawClass::Mask));
    }
    #[test]
    fn curved_bsp_receivers_smooth_only_matching_shared_edges() {
        use crate::renderer::StaticAoLightmapTriangle;
        let a = StaticAoLightmapTriangle {
            material: 7,
            texture: 0,
            owner: 1,
            positions: [[0.0, 0.0, 0.0], [0.0, 0.0, 2.0], [2.0, 0.0, 0.0]],
            normals: [[0.0, 1.0, 0.0]; 3],
            uvs: [[0.1, 0.1], [0.2, 0.1], [0.1, 0.2]],
        };
        let b = StaticAoLightmapTriangle {
            material: 7,
            texture: 1, // Different chart is fine if geometry/material connect.
            owner: 2,
            positions: [[0.0, 0.0, 2.0], [0.0, 0.0, 0.0], [-1.0, 0.0, 1.0]],
            normals: [[0.5, 0.866_025_4, 0.0]; 3],
            uvs: [[0.7, 0.7], [0.8, 0.7], [0.8, 0.8]],
        };
        let mut curved = vec![a.clone(), b.clone()];
        assert!(smooth_sun_receiver_normals(&mut curved) >= 4);
        // Both ends of the *shared* edge get a gradual lighting normal.
        assert!(curved[0].normals[0][0] > 0.05);
        assert!(curved[1].normals[1][0] < 0.5);
        // Vertices outside the shared edge retain their flat normals.
        assert_eq!(curved[0].normals[2], a.normals[2]);

        let mut different_material = vec![a.clone(), StaticAoLightmapTriangle {
            material: 8, ..b.clone()
        }];
        assert_eq!(smooth_sun_receiver_normals(&mut different_material), 0);
        assert_eq!(different_material[0].normals, a.normals);

        let mut hard_corner = vec![a.clone(), StaticAoLightmapTriangle {
            normals: [[1.0, 0.0, 0.0]; 3], ..b
        }];
        assert_eq!(smooth_sun_receiver_normals(&mut hard_corner), 0);
        assert_eq!(hard_corner[0].normals, a.normals);
    }

    #[test]
    fn auto_smooth_welds_by_position_without_chaining_hard_corners() {
        use crate::renderer::StaticAoLightmapTriangle;
        let facet = |positions: [[f32; 3]; 3], material, uv: f32| {
            let p = positions.map(Vec3::from_array);
            let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize();
            StaticAoLightmapTriangle {
                material, texture: 0, owner: material,
                positions, normals: [n.to_array(); 3],
                uvs: [[uv, uv], [uv + 0.1, uv], [uv, uv + 0.1]],
            }
        };
        // Two facets 30 degrees apart (different materials, no shared edge
        // record needed) and a wall at 90 degrees sharing the same vertex.
        let s = 30f32.to_radians();
        let a = facet([[0.0, 0.0, 0.0], [0.0, 0.0, 2.0], [2.0, 0.0, 0.0]], 1, 0.1);
        let b = facet([[0.0, 0.0, 2.0], [0.0, 0.0, 0.0], [-2.0 * s.cos(), -2.0 * s.sin(), 1.0]], 2, 0.5);
        let wall = facet([[0.0, 0.0, 0.0], [0.0, -2.0, 0.0], [0.0, 0.0, 2.0]], 3, 0.8);
        let mut triangles = vec![a.clone(), b, wall.clone()];
        assert!(auto_smooth_sun_receiver_normals(&mut triangles) >= 4);
        let smoothed = Vec3::from_array(triangles[0].normals[0]);
        let flat = Vec3::from_array(a.normals[0]);
        assert!(smoothed.dot(flat) < 0.999 && smoothed.dot(flat) > 0.9);
        // The 90-degree wall is not joined to either facet.
        assert_eq!(triangles[2].normals, wall.normals);
        // Unconnected UV islands are separate charts.
        let charts = triangle_charts(&triangles);
        assert_eq!(charts.len(), 3);
        assert!(charts[0] != charts[1] && charts[1] != charts[2]);
        // The curved origin rises above the flat facet's interior.
        let mut bent = a.clone();
        bent.normals = [[0.3, 1.0, 0.0], [0.0, 1.0, 0.3], [-0.3, 1.0, 0.0]]
            .map(|n| Vec3::from_array(n).normalize().to_array());
        let origin = terminator_origin(&bent, [1.0 / 3.0; 3]);
        assert!(origin.y > 0.0);
        assert_eq!(terminator_origin(&a, [1.0 / 3.0; 3]).y, 0.0);
    }

    #[test]
    fn output_resolution_is_independent_and_bounded() {
        assert_eq!(output_sizes(&[[128, 128]], 8192), vec![[512, 512]]);
        assert_eq!(output_sizes(&[[2048, 2048]], 8192), vec![[2048, 2048]]);
        assert_eq!(output_sizes(&[[1024, 1024]], 2048), vec![[2048, 2048]]);
    }
    #[test]
    fn lightmap_sun_traces_static_roofs_and_cancels_obsolete_bakes() {
        let text = r#"{
"classname" "worldspawn"
{
( 0 0 0 ) ( 0 64 0 ) ( 0 0 64 ) test/wall 0 0 0 1 1
( 64 0 0 ) ( 64 0 64 ) ( 64 64 0 ) test/wall 0 0 0 1 1
( 0 0 0 ) ( 0 0 64 ) ( 64 0 0 ) test/wall 0 0 0 1 1
( 0 64 0 ) ( 64 64 0 ) ( 0 64 64 ) test/wall 0 0 0 1 1
( 0 0 0 ) ( 64 0 0 ) ( 0 64 0 ) test/floor 0 0 0 1 1
( 0 0 64 ) ( 0 64 64 ) ( 64 0 64 ) test/ceiling 0 0 0 1 1
}
}"#;
        let map = scene::prepare_source(
            &std::env::temp_dir().join("sun-lightmap-test"),
            None,
            &scene::MapSource::MapEditPreview {
                path: "sun.map".into(),
                text: text.into(),
            },
        )
        .unwrap();
        let mut floor = map.batches[0].clone();
        floor.vertices = 0..3;
        floor.lightmap = Some(0);
        floor.pipeline.class = DrawClass::Opaque;
        floor.pipeline.depth_equal = false;
        floor.alpha_cutoff = 0.0;
        let mut roof = floor.clone();
        roof.vertices = 3..6;
        roof.lightmap = None;
        let mut sky = roof.clone();
        sky.vertices = 6..9;
        sky.pipeline.class = DrawClass::Sky;
        sky.tc_gen = jka_assets::shader::TcGen::Base;
        let positions = [
            [0.0, 0.0, 0.0],
            [100.0, 0.0, 0.0],
            [0.0, 0.0, 100.0],
            [0.0, 50.0, 0.0],
            [50.0, 50.0, 0.0],
            [0.0, 50.0, 50.0],
            [-100.0, 100.0, -100.0],
            [300.0, 100.0, -100.0],
            [-100.0, 100.0, 300.0],
        ];
        let vertices = Arc::new(
            positions
                .into_iter()
                .enumerate()
                .map(|(i, position)| GpuVertex {
                    position,
                    normal: [0.0, 1.0, 0.0],
                    uv: [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]][i % 3],
                    lightmap_uv: [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]][i % 3],
                    color: [1.0; 4],
                    alpha_cutoff: 0.0,
                })
                .collect(),
        );
        let mapping = (0..vertices.len() as u32).collect();
        let geometry = Geometry::new(
            vertices,
            vec![floor.clone(), roof, sky],
            Arc::new(vec![]),
            &map.material_debug,
            3,
            mapping,
        );
        let key = Key {
            direction: [0.0, -1.0, 0.0],
            cloud_coverage_steps: 0,
            visibility: SunVisibilityMode::SkyPortals,
            models: true,
            tinted: false,
            sizes: vec![[32, 32]],
            bake_sun: true,
            aa: 0,
            normals: 1,
            terminator: false,
            debug_layers: 0,
            sky: 0,
            sky_ab: 0,
            sky_sizes: Vec::new(),
            local_sources: 0,
            local_sizes: Vec::new(),
            locals: Vec::new(),
        };
        assert_eq!(bake_worker_count(1, 1), 1);
        assert_eq!(bake_worker_count(8, 1), 8);
        assert_eq!(bake_worker_count(32, 2), 32);
        assert_eq!(automatic_bake_workers(16, 1, 0), 11);
        assert_eq!(automatic_bake_workers(16, 2, 0), 10);
        assert_eq!(automatic_bake_workers(16, 1, 3), 8);
        assert_eq!(automatic_bake_workers(4, 1, 0), 1);
        assert_eq!(automatic_bake_workers(1, 2, 8), 1);
        assert!(bake_worker_count(0, 1) >= 1);
        assert!(bake_worker_count(0, 2) >= 1);
        let cancel = AtomicBool::new(false);
        let progress = BakeProgress::default();
        let pages = trace(&geometry, &key, &cancel, &progress).unwrap().sun;
        assert_eq!(pages[0][4 * 32 + 4], 0.0, "roof blocks the sun");
        assert_eq!(pages[0][4 * 32 + 24], 1.0, "open floor receives the sun");
        let cloudy_key = Key { cloud_coverage_steps: 20, ..key.clone() };
        let cloudy = trace(&geometry, &cloudy_key, &cancel, &progress).unwrap().sun;
        assert!(
            cloudy[0].iter().zip(&cloudy[0][32 * 32..]).all(|(flux, facing)| flux <= &(facing + 1e-6)),
            "angular visibility cannot exceed the unoccluded central cosine"
        );
        assert!(
            cloudy[0][4 * 32 + 12] > 0.0 && cloudy[0][4 * 32 + 12] < 1.0,
            "an overcast roof must produce a geometric penumbra near its edge"
        );
        assert!(
            pages[0][..32 * 32]
                .iter()
                .zip(&pages[0][32 * 32..])
                .all(|(incident, cosine)| incident <= &(cosine + 1e-6)),
            "chart padding must not add sunlight above its unoccluded cosine"
        );
        assert_eq!(
            pages[0][32 * 32 + 4 * 32 + 4],
            1.0,
            "unoccluded cosine survives shadowing"
        );
        let lamp = scene::DynamicLight {
            position: [10.0, 75.0, 10.0],
            color: [1.0, 0.0, 0.0],
            radius: 200.0,
            intensity: 1.0,
            shadow_power: 0.0,
            falloff: scene::DynamicLightFalloff::Smooth,
            surface_lighting: true,
            emitter_normal: [0.0; 3],
            emitter_two_sided: false,
            angle_attenuation: true,
            angle_scale: 0.0,
            extra_distance: 0.0,
            shader_promotion_mask: 1,
        };
        let mut local_key = key.clone();
        local_key.local_sizes = vec![[32, 32]];
        local_key.locals = vec![
            (0, lamp),
            (
                1,
                scene::DynamicLight {
                    position: [10.0, 25.0, 10.0],
                    ..lamp
                },
            ),
        ];
        let local = trace(&geometry, &local_key, &cancel, &progress).unwrap().local;
        let receiver = 4 * 32 + 4;
        assert_eq!(local[0][receiver], 0.0, "roof blocks the upper lamp");
        assert!(
            local[0][32 * 32 + receiver] > 0.0,
            "lower lamp reaches the receiver independently"
        );
        assert_eq!(local[0].len(),32*32*5);
        assert_eq!(local[0][32*32*2+receiver],local[0][32*32+receiver],"aggregate red irradiance is baked once");
        assert_eq!(local[0][32*32*3+receiver],0.0,"red lights add no green irradiance");
        floor.alpha_cutoff = -0.5;
        floor.animation = None;
        floor.tc_mods = vec![jka_assets::shader::TcMod::Scale(2.0, 2.0)];
        assert!(!moving_static_material(
            &floor,
            SunVisibilityMode::SkyPortals
        ));
        floor.tc_mods = vec![jka_assets::shader::TcMod::Scroll(1.0, 0.0)];
        assert!(
            moving_static_material(&floor, SunVisibilityMode::SkyPortals),
            "negative alphaFunc must stay live too"
        );
        cancel.store(true, Ordering::Relaxed);
        assert!(trace(&geometry, &key, &cancel, &progress).is_err());
    }
}

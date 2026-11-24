//! JKA geometry adapter for Fluid Ninja Snow painter foot contacts.
//!
//! The authored BSP and collision are never displaced. A small independent
//! annular mesh supplies the footprint rim silhouette lost without UE Landscape
//! tessellation. The source footprint remains the private Ninja painter;
//! this bounded berm *presentation* copies Snowflow's existing berm profile,
//! not its simulation, field, tessellated shell or projected decals.
use super::{
    aabb_intersects_clip_frustum, hidden_bsp_model_surface, GpuVertex,
    Mat4, Renderer, Vec2, Vec3, WorldGpu, WorldPipelineBinding,
    WorldPipelineVariant, Arc,
};
use super::environment::snow::{SnowSource, spawn_snow_shell_job};
use crate::surface_deformation::{SurfaceDeformationKind, SurfaceDeformationStamp, SNOW_SHELL_CHUNK_SIZE};
use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::mpsc;
use wgpu::util::DeviceExt;

const MAX_BERM_STAMPS: usize = 48;
const SEGMENTS: usize = 20;
// Source Snowflow foot size = 0.10m, 1.7 longitudinal elongation.
const FOOT_RADIUS_UNITS: f32 = 0.10 * (100.0 / 2.54);
const BERM_HEIGHT_UNITS: f32 = 2.0; // Explicit BSP-adaptation cap; no terrain lift.
const BORDER_BIAS_UNITS: f32 = 0.025;
const RINGS: [f32; 7] = [0.55, 0.70, 0.85, 1.04, 1.20, 1.40, 1.85];

#[derive(Default)]
pub(in crate::renderer) struct NinjaSnowBermState {
    source: Option<Arc<SnowSource>>,
    vertex: Option<wgpu::Buffer>,
    draws: Vec<BermDraw>,
    last_version: Option<u64>,
    pending: Option<mpsc::Receiver<BermBuild>>,
}

struct BermDraw {
    batch: usize,
    vertices: Range<u32>,
    min: [f32; 3],
    max: [f32; 3],
}
struct BermBuild {
    version: u64,
    vertices: Vec<GpuVertex>,
    draws: Vec<BermDraw>,
}

impl Renderer {
    pub(in crate::renderer) fn prepare_ninja_snow_berms(&mut self, draw_world: bool) -> bool {
        let active = draw_world && jka_ninja::INCLUDED
            && self.surface_deformation.mode().ninja_profile() == Some(jka_ninja::SnowProfile::Composite)
            && self.world.as_ref().is_some_and(|w| w.snow_shell.has_snow());
        if !active {
            self.ninja_snow_berms = NinjaSnowBermState::default();
            return false;
        }
        let src = self.world.as_ref().expect("snow world").snow_shell.source.clone();
        if self.ninja_snow_berms.source.as_ref().is_none_or(|old| !Arc::ptr_eq(old, &src)) {
            self.ninja_snow_berms = NinjaSnowBermState::default();
            self.ninja_snow_berms.source = Some(src.clone());
        }
        let state = &mut self.ninja_snow_berms;
        if let Some(job) = &state.pending {
            if let Ok(build) = job.try_recv() {
                // Retire the old buffer only once the next geometry is ready.
                state.vertex = if build.vertices.is_empty() { None } else {
                    Some(self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("JKA Ninja local snow berm ring vertices"),
                        contents: bytemuck::cast_slice(&build.vertices),
                        usage: wgpu::BufferUsages::VERTEX,
                    }))
                };
                state.draws = build.draws;
                state.last_version = Some(build.version);
                state.pending = None;
            }
        }
        let version = self.surface_deformation.ninja_berm_stamp_version();
        if state.pending.is_none() && state.last_version != Some(version) {
            let stamps = self.surface_deformation.ninja_berm_stamps();
            if stamps.is_empty() {
                state.vertex = None;
                state.draws.clear();
                state.last_version = Some(version);
            } else {
                let copied = stamps.iter().rev().take(MAX_BERM_STAMPS).copied().collect::<Vec<_>>();
                let (tx, rx) = mpsc::channel();
                spawn_snow_shell_job(move || {
                    let build = build_berms(&src, &copied, version);
                    let _ = tx.send(build);
                });
                state.pending = Some(rx);
            }
        }
        state.vertex.is_some() && !state.draws.is_empty()
    }
}

pub(in crate::renderer) fn draw_ninja_snow_berms<'a>(
    pass: &mut wgpu::RenderPass<'a>,
    state: &'a NinjaSnowBermState,
    world: &'a WorldGpu,
    pipelines: &'a WorldPipelineVariant,
    view_proj: Mat4,
    draw_bsp_model_surfaces: bool,
    current_pipeline: &mut Option<WorldPipelineBinding>,
) {
    let Some(vertices) = &state.vertex else { return; };
    // The source snow shell's ID stream is a valid u32::MAX per-vertex fallback.
    // No Snowflow GPU geometry/simulation is read or drawn.
    pass.set_vertex_buffer(0, vertices.slice(..));
    pass.set_vertex_buffer(1, world.snow_shell.legacy_dlight_surface_id_buffer.slice(..));
    for draw in &state.draws {
        if !aabb_intersects_clip_frustum(draw.min, draw.max, view_proj) { continue; }
        let Some(batch) = world.coarse_batches.get(draw.batch) else { continue; };
        if hidden_bsp_model_surface(draw_bsp_model_surfaces, batch, &world.material_debug) { continue; }
        let (binding, pipeline) = pipelines.base_pipeline(batch);
        if *current_pipeline != Some(binding) {
            pass.set_pipeline(pipeline);
            *current_pipeline = Some(binding);
        }
        pass.set_bind_group(1, &batch.bind_group, &[]);
        // Instance zero: *not* Snowflow's shell instance 1. The base terrain
        // shader keeps the existing lightmap, UVs, and Ninja snow material.
        pass.draw(draw.vertices.clone(), 0..1);
    }
    pass.set_vertex_buffer(0, world.vertex_buffer.slice(..));
    pass.set_vertex_buffer(1, world.legacy_dlight_surface_id_buffer.slice(..));
    pass.set_index_buffer(world.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
    *current_pipeline = None;
}

fn build_berms(source: &SnowSource, stamps: &[SurfaceDeformationStamp], version: u64) -> BermBuild {
    let mut per_batch = BTreeMap::<usize, Vec<GpuVertex>>::new();
    for stamp in stamps {
        if stamp.kind != SurfaceDeformationKind::Foot { continue; }
        let forward = Vec2::new(stamp.forward[0], stamp.forward[2]).normalize_or_zero();
        let forward = if forward.length_squared() > 0.0 { forward } else { Vec2::X };
        let right = Vec2::new(-forward.y, forward.x);
        let c = Vec2::new(stamp.position[0], stamp.position[2]);
        // Reuse Snowflow's ellipse/rim *shape*, not its displacement solver.
        let rings = RINGS.map(|radius| {
            (0..SEGMENTS).map(|i| {
                let a = (i as f32) * std::f32::consts::TAU / SEGMENTS as f32;
                let pos = c + FOOT_RADIUS_UNITS * radius * (1.7 * a.cos() * forward + a.sin() * right);
                let d = (radius - 1.04) * 3.4;
                let bump = BERM_HEIGHT_UNITS * (-d * d).exp();
                sample_snow_surface(source, pos, stamp.position[1]).map(|(batch, mut vertex)| {
                    vertex.position[1] += bump + BORDER_BIAS_UNITS;
                    (batch, vertex)
                })
            }).collect::<Vec<_>>()
        });
        for ring in 0..RINGS.len()-1 {
            for i in 0..SEGMENTS {
                let j = (i+1) % SEGMENTS;
                let corners = [rings[ring][i], rings[ring][j], rings[ring+1][i], rings[ring+1][j]];
                let [Some(a),Some(b),Some(c),Some(d)] = corners else {continue};
                if a.0 != b.0 || a.0 != c.0 || a.0 != d.0 {continue};
                let dst = per_batch.entry(a.0).or_default();
                dst.extend_from_slice(&[a.1,b.1,c.1,b.1,d.1,c.1]);
            }
        }
    }
    let mut vertices = Vec::new();
    let mut draws = Vec::new();
    for (batch, group) in per_batch {
        if group.is_empty() {continue;}
        let start = vertices.len() as u32;
        let mut min = [f32::INFINITY;3];
        let mut max = [f32::NEG_INFINITY;3];
        for vertex in &group {
            for k in 0..3 {min[k] = min[k].min(vertex.position[k]); max[k] = max[k].max(vertex.position[k]);}
        }
        vertices.extend(group);
        draws.push(BermDraw {batch,vertices: start..vertices.len() as u32,min,max});
    }
    BermBuild {version,vertices,draws}
}

fn sample_snow_surface(source: &SnowSource, pos: Vec2, expected_y: f32) -> Option<(usize,GpuVertex)> {
    let key = ((pos.x/SNOW_SHELL_CHUNK_SIZE).floor() as i32, (pos.y/SNOW_SHELL_CHUNK_SIZE).floor() as i32);
    let mut nearest = f32::INFINITY;
    let mut chosen = None;
    for &index in source.cells.get(&key)? {
        let tri = &source.triangles[index as usize];
        let [a,b,c] = tri.vertices;
        let ab = Vec2::new(b.position[0]-a.position[0], b.position[2]-a.position[2]);
        let ac = Vec2::new(c.position[0]-a.position[0], c.position[2]-a.position[2]);
        let det = ab.x*ac.y - ab.y*ac.x;
        if det.abs() < 1.0e-8 {continue;}
        let p = pos - Vec2::new(a.position[0],a.position[2]);
        let v = (p.x*ac.y - p.y*ac.x)/det;
        let w = (ab.x*p.y - ab.y*p.x)/det;
        let u = 1.0-v-w;
        if u < -0.00001 || v < -0.00001 || w < -0.00001 {continue;}
        let y = u*a.position[1]+v*b.position[1]+w*c.position[1];
        let dy = (y-expected_y).abs();
        // Never bridge stacked BSP floors/ceilings or move an unrelated brush.
        if dy > 6.0 || dy >= nearest {continue;}
        nearest=dy;
        let mut vertex = a;
        vertex.position=[pos.x,y,pos.y];
        for k in 0..2 {
            vertex.uv[k] = u*a.uv[k]+v*b.uv[k]+w*c.uv[k];
            vertex.lightmap_uv[k] = u*a.lightmap_uv[k]+v*b.lightmap_uv[k]+w*c.lightmap_uv[k];
        }
        for k in 0..4 {vertex.color[k]=u*a.color[k]+v*b.color[k]+w*c.color[k];}
        let normal = Vec3::from_array(a.normal)*u + Vec3::from_array(b.normal)*v + Vec3::from_array(c.normal)*w;
        vertex.normal=normal.normalize_or_zero().to_array();
        vertex.alpha_cutoff=u*a.alpha_cutoff+v*b.alpha_cutoff+w*c.alpha_cutoff;
        chosen=Some((tri.coarse_batch_index, vertex));
    }
    chosen
}

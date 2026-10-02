//! WGPU port/adaptation of 2Retr0/GodotOceanWaves.
//! GodotOceanWaves is MIT licensed:
//! Copyright (c) 2024 Ethan Truong
//! Permission is hereby granted, free of charge, to any person obtaining a copy of this
//! software and associated documentation files (the "Software"), to deal in the Software
//! without restriction, including without limitation the rights to use, copy, modify,
//! merge, publish, distribute, sublicense, and/or sell copies of the Software, and to
//! permit persons to whom the Software is furnished to do so, subject to the following
//! conditions: the above copyright notice and this permission notice shall be included
//! in all copies or substantial portions of the Software. THE SOFTWARE IS PROVIDED "AS IS",
//! WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
//! WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

const G: f32 = 9.81;
pub mod authoring;
pub mod optics;
pub use authoring::{OceanAuthoring, OceanWind};
pub const OCEAN_CASCADES: usize = 3;
pub const OCEAN_DEFAULT_MAP_SIZE: u32 = 1024;
pub const OCEAN_MAX_SURFACES: usize = 8;
const OCEAN_WINDROW_SIZE: u32 = 256;
const OCEAN_WINDROW_DOMAIN_METERS: f32 = 384.0;
// JKA/jaPRO world scale used by the working C port and its GodotOceanWaves
// clipmap conversion: one metre in the upstream simulation is 40 game units.
pub const OCEAN_UNITS_PER_METER: f32 = 40.0;
// Innermost clipmap cell size for each mesh quality, matching the upstream
// clipmap_high (0.25 m) and clipmap_low (1 m) inner grids.
pub const OCEAN_HIGH_MESH_SPACING_UNITS: f32 = 0.25 * OCEAN_UNITS_PER_METER;
pub const OCEAN_LOW_MESH_SPACING_UNITS: f32 = 1.0 * OCEAN_UNITS_PER_METER;
const DEPTH_METERS: f32 = 20.0;

/// Half-width of every clipmap level, in cells. Each level nests exactly inside
/// the next because a level's hole is half its own cell count at twice the cell
/// size, so `hole * spacing` always equals the finer level's full extent.
const CLIPMAP_HALF_CELLS: i32 = 128;
/// Levels beyond the first double their cell size, so the covered radius is
/// `HALF_CELLS * spacing * 2^(LEVELS-1)`. Six levels at the HIGH 10-unit inner
/// cell reach +/-40960 units, which covers a full-size JKA map from anywhere.
const CLIPMAP_LEVELS: u32 = 6;

/// One clipmap vertex, expressed in the world vertex format so the ocean can be
/// drawn by the existing BSP pipeline instead of needing its own.
///
/// `position.xz` is the grid offset from the clipmap centre in world units,
/// `color.r` is this vertex's own cell size and `color.g` is the parent cell
/// size for vertices that sit on a level's outer boundary (zero elsewhere).
/// Snapping those boundary vertices onto the parent grid in the vertex shader
/// collapses the mismatched half of them into degenerate triangles, which keeps
/// a 2:1 LOD seam watertight without building transition fans.
pub struct OceanClipmap {
    pub vertices: Vec<crate::scene::GpuVertex>,
    pub indices: Vec<u32>,
    pub coarsest_spacing: f32,
}

/// Marks a vertex as belonging to the ocean clipmap. Real BSP vertices carry
/// cached static AO here, which is never negative, so the shader can recognise
/// clipmap geometry from the vertex alone. That matters: the draw call decides
/// whether the clipmap is used, and nothing else in the shader may be able to
/// disagree with it.
pub const OCEAN_CLIPMAP_VERTEX_MARKER: f32 = -1.0;

/// How many clipmap levels a surface needs. Cell size never depends on the size
/// of the water: level 0 always has the quality's own cell size and each level
/// doubles it. What a footprint decides is only how far out the rings must reach.
///
/// The centre is clamped into the footprint and re-snapped to the outermost
/// built level's lattice, so it can sit half that lattice step outside it. The
/// outermost level therefore has to reach `extent + step / 2` from the centre:
/// `HALF * step >= extent + step / 2`. Anything beyond that would be vertices
/// folded onto the footprint edge as zero-area triangles.
fn clipmap_levels_for(base_spacing: f32, extent: f32) -> u32 {
    if !extent.is_finite() {
        return CLIPMAP_LEVELS;
    }
    let reach = CLIPMAP_HALF_CELLS as f32 - 0.5;
    (1..=CLIPMAP_LEVELS)
        .find(|&levels| base_spacing * (1u32 << (levels - 1)) as f32 * reach >= extent)
        .unwrap_or(CLIPMAP_LEVELS)
}

/// Builds the clipmap for one promoted water surface. Everything the vertex
/// shader needs is baked in:
///   position.xz  grid offset from the clipmap centre, in world units
///   position.y   this surface's water plane height
///   uv           footprint minimum (world xz)
///   lightmap_uv  footprint maximum (world xz)
///   color.r      this vertex's own cell size
///   color.g      parent cell size on a level's outer boundary, else 0
///   color.b      centre snap step: this clipmap's outermost built cell size
///   color.a      shape-mask slot in `OceanMasks`, or -1 for none
///   alpha_cutoff the clipmap marker
///
/// Only as many levels are built as the footprint can use. A pool that fits
/// inside level 0 is a single level, and a single level is also trimmed to the
/// cells the footprint can reach.
pub fn build_clipmap(
    base_spacing: f32,
    plane_height: f32,
    footprint_minimum: [f32; 2],
    footprint_maximum: [f32; 2],
    mask_slot: Option<usize>,
) -> OceanClipmap {
    let mask_slot = mask_slot.map_or(-1.0, |slot| slot as f32);
    let extent = (footprint_maximum[0] - footprint_minimum[0])
        .max(footprint_maximum[1] - footprint_minimum[1]);
    let levels = clipmap_levels_for(base_spacing, extent);
    let snap_step = base_spacing * (1u32 << (levels - 1)) as f32;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for level in 0..levels {
        let spacing = base_spacing * (1u32 << level) as f32;
        // Level 0 is solid; every outer level is an annulus whose hole is
        // exactly the extent of the level it nests around.
        let hole = if level == 0 { 0 } else { CLIPMAP_HALF_CELLS / 2 };
        // Only a lone level has no neighbour whose hole it must match, so only
        // it may shrink below the full width.
        let half = if levels == 1 {
            (((extent + spacing * 0.5) / spacing).ceil() as i32).clamp(1, CLIPMAP_HALF_CELLS)
        } else {
            CLIPMAP_HALF_CELLS
        };
        let parent = if level + 1 < levels { spacing * 2.0 } else { 0.0 };
        let side = (half * 2 + 1) as usize;
        let base = u32::try_from(vertices.len()).unwrap_or(u32::MAX);
        for row in 0..side {
            for column in 0..side {
                let cell_x = column as i32 - half;
                let cell_z = row as i32 - half;
                // Only the outermost vertex ring of a level meets a coarser
                // neighbour, so only it needs parent-grid snapping.
                let on_outer_edge = cell_x.abs() == half || cell_z.abs() == half;
                let snap = if on_outer_edge { parent } else { 0.0 };
                vertices.push(crate::scene::GpuVertex {
                    position: [cell_x as f32 * spacing, plane_height, cell_z as f32 * spacing],
                    uv: footprint_minimum,
                    lightmap_uv: footprint_maximum,
                    normal: [0.0, 1.0, 0.0],
                    color: [spacing, snap, snap_step, mask_slot],
                    alpha_cutoff: OCEAN_CLIPMAP_VERTEX_MARKER,
                });
            }
        }
        for row in 0..(half * 2) {
            for column in 0..(half * 2) {
                let cell_x = column - half;
                let cell_z = row - half;
                // Cells strictly inside the hole belong to the finer level.
                if hole > 0 && cell_x >= -hole && cell_x < hole && cell_z >= -hole && cell_z < hole {
                    continue;
                }
                let index = |c: i32, r: i32| base + (r + half) as u32 * side as u32 + (c + half) as u32;
                let a = index(cell_x, cell_z);
                let b = index(cell_x + 1, cell_z);
                let c = index(cell_x, cell_z + 1);
                let d = index(cell_x + 1, cell_z + 1);
                indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
    }
    OceanClipmap {
        vertices,
        indices,
        // The lattice the uniform's clipmap centre is snapped to. It stays the
        // full-design value, not `snap_step`, so every surface of one quality
        // shares one centre however many levels it actually built.
        coarsest_spacing: base_spacing * (1u32 << (CLIPMAP_LEVELS - 1)) as f32,
    }
}

/// Capacity of the shape-mask triangle store, in vec4s. A triangle takes two.
pub const OCEAN_MASK_VEC4S: usize = 512;

/// One world-space xz triangle of a promoted water face.
pub type MaskTriangle = [[f32; 2]; 3];

/// The real outline of each promoted ocean surface. A clipmap is a square
/// grid folded into its footprint's bounding box, so the water fragments it
/// shades are cut back to these triangles; that is what keeps a hole or an
/// L-shape inside contiguous water from being painted over.
///
/// `ranges[slot]` is `[first triangle, triangle count, 0, 0]`. A count of zero
/// means "no mask": the surface fills its whole footprint.
#[derive(Clone, Debug, PartialEq)]
pub struct OceanMasks {
    pub ranges: [[f32; 4]; OCEAN_MAX_SURFACES],
    pub triangles: Vec<[f32; 4]>,
}

impl Default for OceanMasks {
    fn default() -> Self {
        Self { ranges: [[0.0; 4]; OCEAN_MAX_SURFACES], triangles: Vec::new() }
    }
}

impl OceanMasks {
    /// Stores `triangles` for `slot`. Returns false, leaving the slot unmasked,
    /// when they do not fit; the surface then fills its bounding box.
    pub fn push_surface(&mut self, slot: usize, triangles: &[MaskTriangle]) -> bool {
        if slot >= OCEAN_MAX_SURFACES || triangles.is_empty()
            || (self.triangles.len() / 2 + triangles.len()) * 2 > OCEAN_MASK_VEC4S
        {
            return false;
        }
        self.ranges[slot] = [(self.triangles.len() / 2) as f32, triangles.len() as f32, 0.0, 0.0];
        for t in triangles {
            self.triangles.push([t[0][0], t[0][1], t[1][0], t[1][1]]);
            self.triangles.push([t[2][0], t[2][1], 0.0, 0.0]);
        }
        true
    }
}

/// One upward authored water face as the promotion pass sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterFace {
    pub plane: f32,
    pub minimum: [f32; 2],
    pub maximum: [f32; 2],
    /// Faces of different map-authored oceans never merge: each has its own
    /// wave settings.
    pub authored_ocean: Option<usize>,
}

/// Contiguous water on one plane, promoted as a single ocean surface.
#[derive(Clone, Debug, PartialEq)]
pub struct WaterCluster {
    pub plane: f32,
    pub minimum: [f32; 2],
    pub maximum: [f32; 2],
    /// Indices into the face slice handed to `cluster_water_faces`.
    pub members: Vec<usize>,
}

impl WaterCluster {
    pub fn area(&self) -> f32 {
        (self.maximum[0] - self.minimum[0]).max(0.0) * (self.maximum[1] - self.minimum[1]).max(0.0)
    }
}

/// Faces closer than this count as touching. BSP splits land on shared edges.
const WATER_TOUCH_UNITS: f32 = 1.0;

fn faces_touch(a: &WaterFace, b: &WaterFace) -> bool {
    a.authored_ocean == b.authored_ocean
        && (a.plane - b.plane).abs() < WATER_TOUCH_UNITS
        && a.minimum[0] <= b.maximum[0] + WATER_TOUCH_UNITS
        && b.minimum[0] <= a.maximum[0] + WATER_TOUCH_UNITS
        && a.minimum[1] <= b.maximum[1] + WATER_TOUCH_UNITS
        && b.minimum[1] <= a.maximum[1] + WATER_TOUCH_UNITS
}

/// Merges faces that share a plane and touch or overlap, then orders the result
/// biggest first so the surface cap keeps the water that matters most.
///
/// Connectivity is pairwise between faces, never between cluster bounds, so two
/// pools that merely sit inside each other's bounding box stay separate. A
/// cluster's footprint is still the bounding box of its faces; a hole inside
/// contiguous water is therefore not preserved.
pub fn cluster_water_faces(faces: &[WaterFace]) -> Vec<WaterCluster> {
    let mut parent: Vec<usize> = (0..faces.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for i in 0..faces.len() {
        for j in i + 1..faces.len() {
            if faces_touch(&faces[i], &faces[j]) {
                let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                if a != b {
                    parent[b.max(a)] = b.min(a);
                }
            }
        }
    }
    let mut clusters: Vec<WaterCluster> = Vec::new();
    let mut slot_of_root = vec![usize::MAX; faces.len()];
    for (index, face) in faces.iter().enumerate() {
        let r = root(&mut parent, index);
        if slot_of_root[r] == usize::MAX {
            slot_of_root[r] = clusters.len();
            clusters.push(WaterCluster {
                plane: face.plane,
                minimum: face.minimum,
                maximum: face.maximum,
                members: Vec::new(),
            });
        }
        let cluster = &mut clusters[slot_of_root[r]];
        cluster.plane = cluster.plane.max(face.plane);
        for axis in 0..2 {
            cluster.minimum[axis] = cluster.minimum[axis].min(face.minimum[axis]);
            cluster.maximum[axis] = cluster.maximum[axis].max(face.maximum[axis]);
        }
        cluster.members.push(index);
    }
    clusters.sort_by(|a, b| b.area().total_cmp(&a.area()));
    clusters
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CascadeSettings {
    pub tile_length: [f32; 2],
    pub displacement_scale: f32,
    pub normal_scale: f32,
    pub wind_speed: f32,
    pub wind_direction: f32,
    pub fetch_length: f32,
    pub swell: f32,
    pub spread: f32,
    pub detail: f32,
    pub whitecap: f32,
    pub foam_amount: f32,
}

impl CascadeSettings {
    pub const fn upstream_defaults() -> [Self; OCEAN_CASCADES] {
        // Exact values from GodotOceanWaves/main.tscn.
        [
            Self { tile_length:[88.0,88.0], displacement_scale:1.0, normal_scale:1.0, wind_speed:10.0, wind_direction:20.0, fetch_length:150.0, swell:0.8, spread:0.2, detail:1.0, whitecap:0.5, foam_amount:8.0 },
            Self { tile_length:[57.0,57.0], displacement_scale:0.75, normal_scale:1.0, wind_speed:5.0, wind_direction:15.0, fetch_length:150.0, swell:0.8, spread:0.4, detail:1.0, whitecap:0.5, foam_amount:0.0 },
            Self { tile_length:[16.0,16.0], displacement_scale:0.0, normal_scale:0.25, wind_speed:20.0, wind_direction:20.0, fetch_length:550.0, swell:0.8, spread:0.4, detail:1.0, whitecap:0.25, foam_amount:3.0 },
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OceanSettings {
    pub optics: optics::Settings,
    pub bounds: Option<OceanSurface>,
    pub authored: OceanAuthoring,
    pub wind: OceanWind,
    pub map_size: u32,
    pub mesh_quality: u8, // 0 low, 1 high (upstream MeshQuality)
    pub updates_per_second: f32,
    pub water_color: [f32; 3],
    pub foam_color: [f32; 3],
    pub roughness: f32,
    pub normal_strength: f32,
    pub sea_spray: bool,
    pub wind_foam_streaks: bool,
    pub cascades: [CascadeSettings; OCEAN_CASCADES],
}

impl Default for OceanSettings {
    fn default() -> Self {
        Self {
            optics: optics::Settings::default(),
            bounds: None,
            authored: OceanAuthoring::default(),
            wind: OceanWind::default(),
            map_size: OCEAN_DEFAULT_MAP_SIZE,
            mesh_quality: 1,
            updates_per_second: 48.0,
            // GodotOceanWaves water.gd defaults. Godot converts these from
            // sRGB to linear before the shader; the BSP shader does the same.
            water_color: [0.1, 0.15, 0.18],
            foam_color: [0.73, 0.67, 0.62],
            // mat_water.tres overrides the shader's 0.4 default to 0.65.
            roughness: 0.65,
            normal_strength: 1.0,
            sea_spray: true,
            wind_foam_streaks: true,
            cascades: {
                let mut cascades = CascadeSettings::upstream_defaults();
                cascades[2].displacement_scale = 0.08;
                cascades
            },
        }
    }
}

impl OceanSettings {
    pub fn sanitize(mut self) -> Self {
        self.optics = self.optics.sanitize();
        self.authored = self.authored.sanitize();
        self.wind = self.wind.sanitize();
        self.map_size = match self.map_size {
            128 | 256 | 512 | 1024 => self.map_size,
            n if n < 192 => 128,
            n if n < 384 => 256,
            n if n < 768 => 512,
            _ => 1024,
        };
        self.mesh_quality = self.mesh_quality.min(1);
        self.updates_per_second = self.updates_per_second.clamp(0.0, 60.0);
        for value in self.water_color.iter_mut().chain(self.foam_color.iter_mut()) {
            *value = value.clamp(0.0, 1.0);
        }
        self.roughness = self.roughness.clamp(0.0, 1.0);
        self.normal_strength = self.normal_strength.clamp(0.0, 1.0);
        for cascade in &mut self.cascades {
            cascade.tile_length[0] = cascade.tile_length[0].max(0.0001);
            cascade.tile_length[1] = cascade.tile_length[1].max(0.0001);
            cascade.displacement_scale = cascade.displacement_scale.clamp(0.0, 2.0);
            cascade.normal_scale = cascade.normal_scale.clamp(0.0, 2.0);
            cascade.wind_speed = cascade.wind_speed.max(0.0001);
            cascade.wind_direction = cascade.wind_direction.clamp(-360.0, 360.0);
            cascade.fetch_length = cascade.fetch_length.max(0.0001);
            cascade.swell = cascade.swell.clamp(0.0, 2.0);
            cascade.spread = cascade.spread.clamp(0.0, 1.0);
            cascade.detail = cascade.detail.clamp(0.0, 1.0);
            cascade.whitecap = cascade.whitecap.clamp(0.0, 2.0);
            cascade.foam_amount = cascade.foam_amount.clamp(0.0, 10.0);
        }
        self
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CascadeGpu {
    tile: [f32; 4],
    spectrum: [f32; 4],
    shape: [f32; 4],
    foam: [f32; 4],
    seed: [u32; 4],
    time: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct OceanState { values: [u32; 4] }

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct WindrowGpu {
    map_scales: [[f32; 4]; OCEAN_CASCADES],
    wind_time: [f32; 4], // render XZ wind m/s, elapsed seconds, frame delta
    params: [f32; 4], // domain metres, source strength, lifetime seconds, previous layer
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OceanSurface {
    pub plane_height: f32,
    pub minimum: [f32; 2],
    pub maximum: [f32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct OceanEnvironment {
    sun_direction_intensity: [f32; 4],
    sun_color: [f32; 4],
    sky_ambient: [f32; 4],
}

impl Default for OceanEnvironment {
    fn default() -> Self {
        Self {
            // Direction is light travel direction in renderer coordinates.
            sun_direction_intensity: [-0.3, -0.8, -0.2, 1.0],
            sun_color: [1.0, 1.0, 1.0, 1.0],
            // RGB is linear sky irradiance tint; W tells the shader that a real map skybox exists.
            sky_ambient: [0.28, 0.38, 0.52, 0.0],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct OceanRenderUniform {
    pub map_scales: [[f32; 4]; OCEAN_CASCADES],
    pub water_color: [f32; 4],
    pub foam_color: [f32; 4],
    pub ocean_info: [f32; 4], // enabled, map size, JKA units per meter, windrow domain metres
    pub surface: [f32; 4],    // roughness, normal strength, sea spray enabled, wind foam streaks enabled
    pub clipmap: [f32; 4],    // centre x, centre z, coarsest cell size, active windrow layer
    pub sun_direction_intensity: [f32; 4], // xyz light-travel direction, w relative intensity (250 q3map units = 1)
    pub sun_color: [f32; 4],
    pub sky_ambient: [f32; 4], // rgb linear map-sky average, w map has real skybox
    pub spray_info: [f32; 4],  // surface count, total particles, visible lifetime, lifetime randomness
    pub spray_bounds: [[f32; 4]; OCEAN_MAX_SURFACES], // min xz, max xz in world units
    pub spray_planes: [[f32; 4]; OCEAN_MAX_SURFACES], // x plane height, remaining reserved
    pub swell: [f32; 4], // render XZ direction, wave number / metre, amplitude metres
    pub swell_motion: [f32; 4], // angular speed, primary chop, FFT chop, seconds
    pub wind_motion: [f32; 4], // render XZ wind m/s, spray multiplier, reserved
    pub authored_bounds: [f32;4],
    pub authored_plane: [f32;4], // enabled, height, reserved
    pub mask_ranges: [[f32; 4]; OCEAN_MAX_SURFACES], // first triangle, triangle count
    pub mask_triangles: [[f32; 4]; OCEAN_MASK_VEC4S], // per triangle: a.xy b.xy / c.xy
}

pub struct OceanGpu {
    enabled: bool,
    settings: OceanSettings,
    clipmap_center: [f32; 4],
    environment: OceanEnvironment,
    surfaces: [OceanSurface; OCEAN_MAX_SURFACES],
    surface_count: usize,
    masks: std::sync::Arc<OceanMasks>,
    times: [f32; OCEAN_CASCADES],
    elapsed: f32,
    next_update: f32,
    remaining: usize,
    spectrum_dirty: bool,
    params_buffer: wgpu::Buffer,
    params_upload_buffer: wgpu::Buffer,
    _spectrum_buffer: wgpu::Buffer,
    _butterfly_buffer: wgpu::Buffer,
    _fft_buffer: wgpu::Buffer,
    _foam_buffer: wgpu::Buffer,
    _displacement: wgpu::Texture,
    _normal: wgpu::Texture,
    compute_bind_groups: Vec<wgpu::BindGroup>,
    spectrum_pipeline: wgpu::ComputePipeline,
    butterfly_pipeline: wgpu::ComputePipeline,
    modulate_pipeline: wgpu::ComputePipeline,
    fft_pipeline: wgpu::ComputePipeline,
    transpose_pipeline: wgpu::ComputePipeline,
    unpack_pipeline: wgpu::ComputePipeline,
    _windrow_field_buffer: wgpu::Buffer,
    windrow_bind_group: wgpu::BindGroup,
    windrow_pipeline: wgpu::ComputePipeline,
    windrow_params_buffer: wgpu::Buffer,
    windrow_front: usize,
    render_buffer: wgpu::Buffer,
    pub render_bind_group: wgpu::BindGroup,
}

fn jonswap_alpha(wind_speed: f32, fetch_m: f32) -> f32 {
    0.076 * (wind_speed*wind_speed/(fetch_m*G)).powf(0.22)
}
fn jonswap_peak(wind_speed: f32, fetch_m: f32) -> f32 {
    22.0 * (G*G/(wind_speed*fetch_m)).powf(1.0/3.0)
}

pub(crate) fn create_spray_pipeline(
    device: &wgpu::Device,
    camera_layout: &wgpu::BindGroupLayout,
    render_layout: &wgpu::BindGroupLayout,
    surface_format: wgpu::TextureFormat,
    samples: u32,
) -> wgpu::RenderPipeline {
    let spray_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("GodotOceanWaves sea spray port"),
        source: wgpu::ShaderSource::Wgsl(include_str!("ocean_spray.wgsl").into()),
    });
    let spray_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Ocean sea spray pipeline layout"),
        bind_group_layouts: &[Some(camera_layout), Some(render_layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Ocean sea spray pipeline"),
        layout: Some(&spray_layout),
        vertex: wgpu::VertexState {
            module: &spray_shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: &spray_shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

impl OceanGpu {
    pub fn create_inert_render_bind_group(
        device: &wgpu::Device,
        render_layout: &wgpu::BindGroupLayout,
    ) -> wgpu::BindGroup {
        let output_desc = |label: &'static str| wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: OCEAN_CASCADES as u32 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };
        let displacement = device.create_texture(&output_desc("Ocean inert displacement"));
        let normal = device.create_texture(&output_desc("Ocean inert normal"));
        let windrow = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ocean inert wind foam streaks"),
            size: 8,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let spray_albedo = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Ocean inert spray albedo"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let displacement_view = displacement.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let normal_view = normal.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let spray_albedo_view = spray_albedo.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Ocean inert sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let settings = OceanSettings::default();
        let render_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ocean inert render uniform"),
            contents: bytemuck::bytes_of(&make_render_uniform(false, &settings)),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Ocean inert render bind group"),
            layout: render_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&displacement_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&normal_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
                bg_buffer(3, &render_buffer),
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&spray_albedo_view) },
                bg_buffer(5, &windrow),
            ],
        })
    }

    pub fn create_render_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label:Some("Ocean render layout"), entries:&[
                wgpu::BindGroupLayoutEntry{binding:0,visibility:wgpu::ShaderStages::VERTEX|wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Texture{sample_type:wgpu::TextureSampleType::Float{filterable:true},view_dimension:wgpu::TextureViewDimension::D2Array,multisampled:false},count:None},
                wgpu::BindGroupLayoutEntry{binding:1,visibility:wgpu::ShaderStages::VERTEX|wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Texture{sample_type:wgpu::TextureSampleType::Float{filterable:true},view_dimension:wgpu::TextureViewDimension::D2Array,multisampled:false},count:None},
                wgpu::BindGroupLayoutEntry{binding:2,visibility:wgpu::ShaderStages::VERTEX|wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),count:None},
                uniform_layout_entry(3,wgpu::ShaderStages::VERTEX|wgpu::ShaderStages::FRAGMENT),
                wgpu::BindGroupLayoutEntry{binding:4,visibility:wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Texture{sample_type:wgpu::TextureSampleType::Float{filterable:true},view_dimension:wgpu::TextureViewDimension::D2,multisampled:false},count:None},
                wgpu::BindGroupLayoutEntry{binding:5,visibility:wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Buffer{ty:wgpu::BufferBindingType::Storage{read_only:true},has_dynamic_offset:false,min_binding_size:None},count:None},
            ]
        })
    }

    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        render_layout: &wgpu::BindGroupLayout,
        spray_albedo_view: &wgpu::TextureView,
        requested_settings: OceanSettings,
    ) -> Self {
        let settings = requested_settings.sanitize();
        let map_size = settings.map_size;
        let stages = map_size.ilog2();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("GodotOceanWaves faithful compute port"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ocean_compute.wgsl").into()),
        });
        let storage = |label: &str, size: u64| device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label), size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation:false
        });
        let n = u64::from(map_size);
        let params_size=(std::mem::size_of::<CascadeGpu>()*OCEAN_CASCADES) as u64;
        let params_buffer = storage("Ocean cascade parameters", params_size);
        let params_upload_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label:Some("Ocean cascade parameter upload"),
            size:params_size,
            usage:wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation:false,
        });
        let spectrum_buffer = storage("Ocean spectrum", n*n*OCEAN_CASCADES as u64*16);
        let butterfly_buffer = storage("Ocean FFT butterfly", n*u64::from(stages)*16);
        let fft_buffer = storage("Ocean FFT pingpong", n*n*4*2*8);
        let foam_buffer = storage("Ocean persistent foam", n*n*OCEAN_CASCADES as u64*4);
        let output_desc = |label: &'static str| wgpu::TextureDescriptor {
            label: Some(label), size: wgpu::Extent3d { width:map_size,height:map_size,depth_or_array_layers:OCEAN_CASCADES as u32 },
            mip_level_count:1, sample_count:1, dimension:wgpu::TextureDimension::D2,
            format:wgpu::TextureFormat::Rgba16Float,
            usage:wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats:&[],
        };
        let displacement = device.create_texture(&output_desc("Ocean displacement array"));
        let normal = device.create_texture(&output_desc("Ocean normal/foam array"));
        let displacement_view = displacement.create_view(&wgpu::TextureViewDescriptor { dimension:Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let normal_view = normal.create_view(&wgpu::TextureViewDescriptor { dimension:Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });

        // Separate persistent surface-foam field. Two halves of one storage
        // buffer are ping-ponged. Compute reads only the previous half and writes
        // only the next half, while the BSP shader performs the small bilinear
        // sample itself. This avoids texture read/write aliasing across backends.
        let windrow_field_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ocean wind foam streak field"),
            size: u64::from(OCEAN_WINDROW_SIZE) * u64::from(OCEAN_WINDROW_SIZE) * 2 * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });

        let compute_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label:Some("Ocean compute layout"), entries:&[
                storage_entry(0,true), storage_entry(1,false), storage_entry(2,false), storage_entry(3,false), storage_entry(4,false),
                storage_texture_entry(5), storage_texture_entry(6), uniform_entry(7),
            ]
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label:Some("Ocean compute pipeline layout"), bind_group_layouts:&[Some(&compute_layout)], immediate_size:0 });
        let cp = |label:&str, entry:&str| device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label:Some(label), layout:Some(&pipeline_layout), module:&shader, entry_point:Some(entry), compilation_options:Default::default(), cache:None
        });
        let spectrum_pipeline = cp("Ocean spectrum compute","spectrum_compute");
        let butterfly_pipeline = cp("Ocean FFT butterfly","fft_butterfly");
        let modulate_pipeline = cp("Ocean spectrum modulate","spectrum_modulate");
        let fft_pipeline = cp("Ocean Stockham FFT","fft_compute");
        let transpose_pipeline = cp("Ocean FFT transpose","transpose");
        let unpack_pipeline = cp("Ocean FFT unpack","fft_unpack");
        let state_buffers: Vec<_> = (0..OCEAN_CASCADES).map(|cascade| device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label:Some("Ocean cascade state"), contents:bytemuck::bytes_of(&OceanState{values:[cascade as u32,map_size,OCEAN_CASCADES as u32,0]}), usage:wgpu::BufferUsages::UNIFORM
        })).collect();
        let compute_bind_groups = state_buffers.iter().map(|state| device.create_bind_group(&wgpu::BindGroupDescriptor {
            label:Some("Ocean compute bind group"), layout:&compute_layout, entries:&[
                bg_buffer(0,&params_buffer), bg_buffer(1,&spectrum_buffer), bg_buffer(2,&butterfly_buffer), bg_buffer(3,&fft_buffer), bg_buffer(4,&foam_buffer),
                wgpu::BindGroupEntry{binding:5,resource:wgpu::BindingResource::TextureView(&displacement_view)},
                wgpu::BindGroupEntry{binding:6,resource:wgpu::BindingResource::TextureView(&normal_view)},
                bg_buffer(7,state),
            ]
        })).collect::<Vec<_>>();

        let windrow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ocean wind foam streak compute"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ocean_windrow.wgsl").into()),
        });
        let windrow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ocean wind foam streak compute layout"),
            entries: &[
                storage_entry(0, false),
                wgpu::BindGroupLayoutEntry { binding:1, visibility:wgpu::ShaderStages::COMPUTE, ty:wgpu::BindingType::Texture { sample_type:wgpu::TextureSampleType::Float { filterable:true }, view_dimension:wgpu::TextureViewDimension::D2Array, multisampled:false }, count:None },
                wgpu::BindGroupLayoutEntry { binding:2, visibility:wgpu::ShaderStages::COMPUTE, ty:wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count:None },
                uniform_layout_entry(3, wgpu::ShaderStages::COMPUTE),
            ],
        });
        let windrow_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Ocean wind foam streak pipeline layout"),
            bind_group_layouts: &[Some(&windrow_layout)],
            immediate_size: 0,
        });
        let windrow_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Ocean wind foam streak pipeline"),
            layout: Some(&windrow_pipeline_layout),
            module: &windrow_shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let windrow_params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ocean wind foam streak parameters"),
            contents: bytemuck::bytes_of(&WindrowGpu::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let render_uniform = make_render_uniform(false,&settings);
        let render_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label:Some("Ocean render uniform"), contents:bytemuck::bytes_of(&render_uniform), usage:wgpu::BufferUsages::UNIFORM|wgpu::BufferUsages::COPY_DST });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { label:Some("Ocean repeat sampler"), address_mode_u:wgpu::AddressMode::Repeat,address_mode_v:wgpu::AddressMode::Repeat,address_mode_w:wgpu::AddressMode::ClampToEdge,mag_filter:wgpu::FilterMode::Linear,min_filter:wgpu::FilterMode::Linear,mipmap_filter:wgpu::MipmapFilterMode::Linear,..Default::default() });
        let render_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { label:Some("Ocean render bind group"), layout:render_layout, entries:&[
            wgpu::BindGroupEntry{binding:0,resource:wgpu::BindingResource::TextureView(&displacement_view)},
            wgpu::BindGroupEntry{binding:1,resource:wgpu::BindingResource::TextureView(&normal_view)},
            wgpu::BindGroupEntry{binding:2,resource:wgpu::BindingResource::Sampler(&sampler)},
            bg_buffer(3,&render_buffer),
            wgpu::BindGroupEntry{binding:4,resource:wgpu::BindingResource::TextureView(spray_albedo_view)},
            bg_buffer(5, &windrow_field_buffer),
        ]});
        let windrow_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Ocean wind foam streak compute bind group"),
            layout: &windrow_layout,
            entries: &[
                bg_buffer(0, &windrow_field_buffer),
                wgpu::BindGroupEntry { binding:1, resource:wgpu::BindingResource::TextureView(&normal_view) },
                wgpu::BindGroupEntry { binding:2, resource:wgpu::BindingResource::Sampler(&sampler) },
                bg_buffer(3, &windrow_params_buffer),
            ],
        });

        let mut ocean = Self { enabled:false,settings,clipmap_center:[0.0;4],environment:OceanEnvironment::default(),surfaces:[OceanSurface{plane_height:0.0,minimum:[0.0;2],maximum:[0.0;2]};OCEAN_MAX_SURFACES],surface_count:0,masks:std::sync::Arc::new(OceanMasks::default()),times:[120.0,120.0+std::f32::consts::PI,120.0+2.0*std::f32::consts::PI],elapsed:0.0,next_update:0.0,remaining:0,spectrum_dirty:true,params_buffer,params_upload_buffer,_spectrum_buffer:spectrum_buffer,_butterfly_buffer:butterfly_buffer,_fft_buffer:fft_buffer,_foam_buffer:foam_buffer,_displacement:displacement,_normal:normal,compute_bind_groups,spectrum_pipeline,butterfly_pipeline,modulate_pipeline,fft_pipeline,transpose_pipeline,unpack_pipeline,_windrow_field_buffer:windrow_field_buffer,windrow_bind_group,windrow_pipeline,windrow_params_buffer,windrow_front:0,render_buffer,render_bind_group };
        ocean.upload_params(queue,0.0);
        let mut encoder=device.create_command_encoder(&wgpu::CommandEncoderDescriptor{label:Some("Ocean FFT init")});
        {
            let mut pass=encoder.begin_compute_pass(&wgpu::ComputePassDescriptor{label:Some("Ocean butterfly init"),timestamp_writes:None});
            pass.set_pipeline(&ocean.butterfly_pipeline);
            pass.set_bind_group(0,&ocean.compute_bind_groups[0],&[]);
            pass.dispatch_workgroups(map_size/128,stages,1);
        }
        queue.submit(Some(encoder.finish()));
        ocean
    }

    pub fn map_size(&self) -> u32 { self.settings.map_size }

    pub fn set_time(&mut self, seconds: f32) {
        if !seconds.is_finite() { return; }
        if (seconds - self.elapsed).abs() > 0.05 {
            self.elapsed = seconds;
            self.times = std::array::from_fn(|i| seconds + 120.0 + i as f32 * std::f32::consts::PI);
            self.next_update = seconds;
            self.remaining = 0;
        }
    }

    pub fn set_enabled(&mut self, queue:&wgpu::Queue, enabled:bool) {
        self.enabled=enabled;
        self.write_render_uniform(queue);
    }

    fn write_render_uniform(&self, queue:&wgpu::Queue) {
        let mut uniform=make_render_uniform(self.enabled,&self.settings);
        uniform.clipmap=self.clipmap_center;
        uniform.clipmap[3] = self.windrow_front as f32;
        uniform.swell_motion[3] = self.elapsed;
        let wind = self.settings.wind.at(self.elapsed);
        uniform.wind_motion[0] = wind[0] / OCEAN_UNITS_PER_METER;
        uniform.wind_motion[1] = -wind[1] / OCEAN_UNITS_PER_METER;
        apply_scene_context(&mut uniform, self.environment, &self.surfaces, self.surface_count);
        uniform.mask_ranges = self.masks.ranges;
        uniform.mask_triangles[..self.masks.triangles.len()].copy_from_slice(&self.masks.triangles);
        queue.write_buffer(&self.render_buffer,0,bytemuck::bytes_of(&uniform));
    }

    /// Hands the ocean the outlines of the world's promoted surfaces. They are
    /// indexed by the slot baked into each clipmap, which is not the order of
    /// `set_surfaces` (that list drops surfaces an authored ocean covers).
    pub fn set_masks(&mut self, queue: &wgpu::Queue, masks: &std::sync::Arc<OceanMasks>) {
        if std::sync::Arc::ptr_eq(&self.masks, masks) { return; }
        self.masks = std::sync::Arc::clone(masks);
        self.write_render_uniform(queue);
    }

    /// Re-centres the clipmap on the viewer. The centre is snapped to the
    /// coarsest cell size so every level stays aligned to the same lattice;
    /// because the wave field is addressed in world space the surface does not
    /// swim when the grid steps.
    pub fn set_clipmap_center(&mut self, queue:&wgpu::Queue, camera:[f32;3], coarsest_spacing:f32) {
        let step=coarsest_spacing.max(1.0);
        let center=[(camera[0]/step).round()*step,(camera[2]/step).round()*step,step,0.0];
        if center==self.clipmap_center { return; }
        self.clipmap_center=center;
        self.write_render_uniform(queue);
    }

    /// Feeds the FFT water the same authored sun/sky context used by the rest of the map.
    /// `sun_direction` is the renderer-space direction the light travels.
    pub fn set_environment(
        &mut self,
        queue: &wgpu::Queue,
        sun_direction: [f32; 3],
        sun_color: [f32; 3],
        sun_intensity: f32,
        sky_average: [f32; 3],
        has_skybox: bool,
    ) {
        let length = (sun_direction[0]*sun_direction[0] + sun_direction[1]*sun_direction[1] + sun_direction[2]*sun_direction[2]).sqrt();
        let direction = if length > 1.0e-6 {
            [sun_direction[0]/length, sun_direction[1]/length, sun_direction[2]/length]
        } else {
            [-0.3, -0.8, -0.2]
        };
        let environment = OceanEnvironment {
            sun_direction_intensity: [direction[0], direction[1], direction[2], (sun_intensity / 250.0).clamp(0.0, 4.0)],
            sun_color: [sun_color[0].max(0.0), sun_color[1].max(0.0), sun_color[2].max(0.0), 1.0],
            sky_ambient: [sky_average[0].max(0.0), sky_average[1].max(0.0), sky_average[2].max(0.0), if has_skybox { 1.0 } else { 0.0 }],
        };
        if environment == self.environment { return; }
        self.environment = environment;
        self.write_render_uniform(queue);
    }

    pub fn set_surfaces(&mut self, queue: &wgpu::Queue, surfaces: &[OceanSurface]) {
        let count = surfaces.len().min(OCEAN_MAX_SURFACES);
        let mut next = [OceanSurface { plane_height: 0.0, minimum: [0.0; 2], maximum: [0.0; 2] }; OCEAN_MAX_SURFACES];
        next[..count].copy_from_slice(&surfaces[..count]);
        if count == self.surface_count && next == self.surfaces { return; }
        self.surface_count = count;
        self.surfaces = next;
        self.write_render_uniform(queue);
    }

    pub fn set_settings(&mut self, queue:&wgpu::Queue, requested:OceanSettings) {
        let settings=requested.sanitize();
        debug_assert_eq!(settings.map_size,self.settings.map_size,"map-size changes recreate OceanGpu");
        if settings == self.settings { return; }
        let old_updates=self.settings.updates_per_second;
        let spectrum_changed = settings.authored.seed != self.settings.authored.seed
            || settings.authored.direction != self.settings.authored.direction
            || settings.wind != self.settings.wind
            || settings.cascades.iter().zip(self.settings.cascades.iter()).any(|(a,b)| {
            a.tile_length != b.tile_length || a.wind_speed != b.wind_speed || a.wind_direction != b.wind_direction ||
            a.fetch_length != b.fetch_length || a.swell != b.swell || a.spread != b.spread || a.detail != b.detail
        });
        // water.gd preserves the phase of the update scheduler when the UPS
        // slider changes instead of restarting the schedule from "now".
        if settings.updates_per_second != old_updates {
            let old_period=1.0/(old_updates+1e-10);
            let new_period=1.0/(settings.updates_per_second+1e-10);
            self.next_update -= old_period-new_period;
        }
        self.settings=settings;
        self.spectrum_dirty |= spectrum_changed;
        // Whitecap/foam rates and displacement/normal scales do not require a
        // spectrum rebuild, but do need fresh compute/render parameters.
        self.upload_params(queue,0.0);
        self.write_render_uniform(queue);
    }

    fn build_params(&mut self, delta:f32) -> [CascadeGpu;OCEAN_CASCADES] {
        let mut gpu=[CascadeGpu::zeroed();OCEAN_CASCADES];
        for (i, gpu_param) in gpu.iter_mut().enumerate() {
            let s=self.settings.cascades[i]; self.times[i]+=delta;
            let authored = self.settings.authored;
            let weather = self.settings.wind;
            let fetch=s.fetch_length.max(0.0001)*1000.0;
            let wind=if i == 2 { (weather.speed / OCEAN_UNITS_PER_METER).max(0.01) } else { s.wind_speed.max(0.0001) };
            let direction = if i == 2 { weather.direction } else { authored.direction + s.wind_direction };
            let resolved_wind = weather.at(self.elapsed);
            let wind_strength = (resolved_wind[0].hypot(resolved_wind[1]) / 800.0).clamp(0.0, 2.0);
            let derivative_scale = if i == 2 { s.displacement_scale * authored.wind_chop * wind_strength } else {
                s.displacement_scale * authored.amplitude / 128.0 * authored.slosh
                    / (authored.wavelength / 1400.0) * authored.steepness / 0.65
            };
            *gpu_param=CascadeGpu {
                tile:[s.tile_length[0],s.tile_length[1],derivative_scale,0.0],
                spectrum:[jonswap_alpha(wind,fetch),jonswap_peak(wind,fetch),wind,(direction + 180.0).to_radians()],
                shape:[DEPTH_METERS,s.swell,s.detail,s.spread],
                foam:[s.whitecap + wind_strength * 0.15,delta*s.foam_amount*7.5*authored.foam*(0.25+wind_strength),delta/authored.foam_lifetime,0.0],
                seed:[authored.seed,authored.seed ^ (i as u32).wrapping_mul(0x9e3779b9),0,0], time:[self.times[i],0.0,0.0,0.0]
            };
        }
        gpu
    }

    fn upload_params(&mut self, queue:&wgpu::Queue, delta:f32) {
        let gpu=self.build_params(delta);
        queue.write_buffer(&self.params_buffer,0,bytemuck::cast_slice(&gpu));
    }

    fn stage_params(&mut self, queue:&wgpu::Queue, encoder:&mut wgpu::CommandEncoder, delta:f32) {
        let gpu=self.build_params(delta);
        queue.write_buffer(&self.params_upload_buffer,0,bytemuck::cast_slice(&gpu));
        encoder.copy_buffer_to_buffer(
            &self.params_upload_buffer,
            0,
            &self.params_buffer,
            0,
            std::mem::size_of_val(&gpu) as u64,
        );
    }

    /// Whether this ocean will draw sea spray, i.e. needs the shared spray pipeline.
    pub fn spray_active(&self) -> bool {
        self.enabled && self.settings.sea_spray
    }

    pub fn draw_spray<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera_bind_group: &'a wgpu::BindGroup,
        spray_pipeline: Option<&'a wgpu::RenderPipeline>,
    ) {
        if !self.enabled || !self.settings.sea_spray || self.surface_count == 0 {
            return;
        }
        let Some(spray_pipeline) = spray_pipeline else {
            return;
        };
        pass.set_pipeline(spray_pipeline);
        pass.set_bind_group(0, camera_bind_group, &[]);
        pass.set_bind_group(1, &self.render_bind_group, &[]);
        // Upstream uses one 32,768-particle GPUParticles3D emitter. Keep that
        // total fixed and distribute particles across promoted water surfaces.
        pass.draw(0..6, 0..32_768);
    }

    pub fn encode_frame(&mut self, queue:&wgpu::Queue, encoder:&mut wgpu::CommandEncoder, delta:f32) {
        if !self.enabled { return; }
        let delta=delta.max(0.0);
        let now=self.elapsed;
        let updates=self.settings.updates_per_second;
        let target=1.0/(updates+1e-10);
        let due=updates==0.0 || now>=self.next_update;
        if due {
            // Upstream completes any stale cascades first, then schedules one
            // cascade per frame to spread FFT cost over time.
            if self.remaining>0 {
                // Match WaveGenerator.update(): finish stale cascades in ascending
                // order before scheduling the next pass.
                let stale=self.remaining;
                for i in 0..stale { self.encode_one(encoder,i); }
                self.remaining=0;
            }
            // Exact water.gd scheduling semantics: test against the current
            // accumulated time, update, then advance `time` by the frame delta.
            let update_delta=if updates==0.0 { delta } else { target+(now-self.next_update) };
            self.next_update=now+target;
            // Keep the old parameter buffer alive for the stale-cascade work
            // encoded above. Queue::write_buffer happens before the submitted
            // command buffer, so write a staging buffer and copy it *here* in
            // command order; the newly scheduled cascades then see the new
            // times/foam rates exactly where upstream WaveGenerator.update()
            // changes parameter sets.
            self.stage_params(queue,encoder,update_delta);
            self.remaining=OCEAN_CASCADES;
            if self.spectrum_dirty {
                let n=self.settings.map_size;
                {
                    let mut pass=encoder.begin_compute_pass(&wgpu::ComputePassDescriptor{label:Some("Ocean spectrum regenerate"),timestamp_writes:None});
                    pass.set_pipeline(&self.spectrum_pipeline);
                    pass.set_bind_group(0,&self.compute_bind_groups[0],&[]);
                    pass.dispatch_workgroups(n/16,n/16,OCEAN_CASCADES as u32);
                }
                self.spectrum_dirty=false;
            }
        }
        if self.remaining>0 {
            // Match WaveGenerator._process(): decrement first, then update that
            // cascade index, so a newly scheduled 3-cascade pass runs 2, 1, 0.
            self.remaining-=1;
            self.encode_one(encoder,self.remaining);
        }
        self.elapsed=now+delta;
        if self.settings.wind_foam_streaks {
            self.encode_windrow(queue, encoder, delta);
        }
        // Publish the layer produced above. Keeping the feature bit in the same
        // uniform means disabling it is an exact render A/B and skips compute.
        self.write_render_uniform(queue);
    }

    fn encode_windrow(&mut self, queue:&wgpu::Queue, encoder:&mut wgpu::CommandEncoder, delta:f32) {
        let wind = self.settings.wind.at(self.elapsed);
        let render_wind = [wind[0] / OCEAN_UNITS_PER_METER, -wind[1] / OCEAN_UNITS_PER_METER];
        let mut map_scales = [[0.0;4]; OCEAN_CASCADES];
        for (i, scale) in map_scales.iter_mut().enumerate() {
            let cascade = self.settings.cascades[i];
            let spatial = if i == 2 { 1.0 } else { self.settings.authored.wavelength / 1400.0 };
            *scale = [
                1.0/(cascade.tile_length[0]*spatial),
                1.0/(cascade.tile_length[1]*spatial),
                0.0, 0.0
            ];
        }
        let params = WindrowGpu {
            map_scales,
            wind_time: [render_wind[0], render_wind[1], self.elapsed, delta.min(0.1)],
            params: [OCEAN_WINDROW_DOMAIN_METERS, 1.0, 12.0, self.windrow_front as f32],
        };
        queue.write_buffer(&self.windrow_params_buffer, 0, bytemuck::bytes_of(&params));
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Ocean wind foam streak transport"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.windrow_pipeline);
            pass.set_bind_group(0, &self.windrow_bind_group, &[]);
            pass.dispatch_workgroups(OCEAN_WINDROW_SIZE.div_ceil(8), OCEAN_WINDROW_SIZE.div_ceil(8), 1);
        }
        self.windrow_front = 1 - self.windrow_front;
    }

    fn encode_one(&self, encoder:&mut wgpu::CommandEncoder, cascade:usize) {
        let n=self.settings.map_size;
        let bind=&self.compute_bind_groups[cascade];

        // These stages have true storage read-after-write dependencies.  Keep
        // them in distinct WebGPU compute passes so wgpu inserts the required
        // pass-boundary memory/usage synchronization.  The upstream generator
        // likewise has an explicit barrier in the FFT chain (between transpose
        // and the second row FFT); a single pass here allowed later dispatches
        // to observe stale scratch data, which collapsed displacement and drove
        // the Jacobian foam accumulator toward saturation.
        {
            let mut pass=encoder.begin_compute_pass(&wgpu::ComputePassDescriptor{label:Some("GodotOceanWaves spectrum modulate"),timestamp_writes:None});
            pass.set_bind_group(0,bind,&[]);
            pass.set_pipeline(&self.modulate_pipeline);
            pass.dispatch_workgroups(n/16,n/16,1);
        }
        {
            let mut pass=encoder.begin_compute_pass(&wgpu::ComputePassDescriptor{label:Some("GodotOceanWaves FFT rows"),timestamp_writes:None});
            pass.set_bind_group(0,bind,&[]);
            pass.set_pipeline(&self.fft_pipeline);
            pass.dispatch_workgroups(1,n,4);
        }
        {
            let mut pass=encoder.begin_compute_pass(&wgpu::ComputePassDescriptor{label:Some("GodotOceanWaves FFT transpose"),timestamp_writes:None});
            pass.set_bind_group(0,bind,&[]);
            pass.set_pipeline(&self.transpose_pipeline);
            pass.dispatch_workgroups(n/32,n/32,4);
        }
        {
            let mut pass=encoder.begin_compute_pass(&wgpu::ComputePassDescriptor{label:Some("GodotOceanWaves FFT columns"),timestamp_writes:None});
            pass.set_bind_group(0,bind,&[]);
            pass.set_pipeline(&self.fft_pipeline);
            pass.dispatch_workgroups(1,n,4);
        }
        {
            let mut pass=encoder.begin_compute_pass(&wgpu::ComputePassDescriptor{label:Some("GodotOceanWaves FFT unpack"),timestamp_writes:None});
            pass.set_bind_group(0,bind,&[]);
            pass.set_pipeline(&self.unpack_pipeline);
            pass.dispatch_workgroups(n/16,n/16,1);
        }
    }
}

fn srgb_channel_to_linear(c:f32)->f32 { if c<=0.04045 { c/12.92 } else { ((c+0.055)/1.055).powf(2.4) } }
fn linear_color(rgb:[f32;3])->[f32;4] { [srgb_channel_to_linear(rgb[0]),srgb_channel_to_linear(rgb[1]),srgb_channel_to_linear(rgb[2]),1.0] }
fn make_render_uniform(enabled:bool,settings:&OceanSettings)->OceanRenderUniform {
    let mut scales=[[0.0;4];OCEAN_CASCADES];
    for (i,scale) in scales.iter_mut().enumerate() {
        let cascade=settings.cascades[i];
        let spatial = if i == 2 { 1.0 } else { settings.authored.wavelength / 1400.0 };
        let strength = if i == 2 { settings.authored.wind_chop * (settings.wind.speed / 800.0).clamp(0.0, 2.0) } else { settings.authored.amplitude / 128.0 * settings.authored.slosh };
        let displacement = cascade.displacement_scale * strength;
        *scale=[1.0/(cascade.tile_length[0]*spatial),1.0/(cascade.tile_length[1]*spatial),displacement,cascade.normal_scale*strength/spatial];
    }
    OceanRenderUniform {
        map_scales:scales,
        water_color:linear_color(settings.water_color),
        foam_color:linear_color(settings.foam_color),
        ocean_info:[if enabled{1.0}else{0.0},settings.map_size as f32,OCEAN_UNITS_PER_METER,OCEAN_WINDROW_DOMAIN_METERS],
        surface:[settings.roughness,settings.normal_strength,if settings.sea_spray { 1.0 } else { 0.0 },if settings.wind_foam_streaks { 1.0 } else { 0.0 }],
        clipmap:[0.0,0.0,0.0,0.0],
        sun_direction_intensity:[-0.3,-0.8,-0.2,1.0],
        sun_color:[1.0,1.0,1.0,1.0],
        sky_ambient:[0.28,0.38,0.52,0.0],
        spray_info:[0.0,32768.0,3.0,0.25],
        spray_bounds:[[0.0;4];OCEAN_MAX_SURFACES],
        spray_planes:[[0.0;4];OCEAN_MAX_SURFACES],
        swell: {
            let angle = settings.authored.direction.to_radians();
            [angle.cos(), -angle.sin(), std::f32::consts::TAU * OCEAN_UNITS_PER_METER / settings.authored.wavelength,
                settings.authored.amplitude / OCEAN_UNITS_PER_METER * (1.0-settings.authored.slosh)]
        },
        swell_motion: [std::f32::consts::TAU * settings.authored.speed / settings.authored.wavelength,
            0.9 * settings.authored.steepness / (settings.authored.amplitude * std::f32::consts::TAU / settings.authored.wavelength).max(0.00001),
            settings.authored.steepness / 0.65, 0.0],
        wind_motion: [0.0,0.0,settings.authored.spray,0.0],
        authored_bounds: settings.bounds.map(|b| [b.minimum[0],b.minimum[1],b.maximum[0],b.maximum[1]]).unwrap_or([0.0;4]),
        authored_plane: settings.bounds.map(|b| [1.0,b.plane_height,0.0,0.0]).unwrap_or([0.0;4]),
        mask_ranges: [[0.0; 4]; OCEAN_MAX_SURFACES],
        mask_triangles: [[0.0; 4]; OCEAN_MASK_VEC4S],
    }
}

fn apply_scene_context(
    uniform: &mut OceanRenderUniform,
    environment: OceanEnvironment,
    surfaces: &[OceanSurface; OCEAN_MAX_SURFACES],
    surface_count: usize,
) {
    uniform.sun_direction_intensity = environment.sun_direction_intensity;
    uniform.sun_color = environment.sun_color;
    uniform.sky_ambient = environment.sky_ambient;
    uniform.spray_info[0] = surface_count.min(OCEAN_MAX_SURFACES) as f32;
    for (index, surface) in surfaces.iter().take(surface_count.min(OCEAN_MAX_SURFACES)).enumerate() {
        uniform.spray_bounds[index] = [surface.minimum[0], surface.minimum[1], surface.maximum[0], surface.maximum[1]];
        uniform.spray_planes[index] = [surface.plane_height, 0.0, 0.0, 0.0];
    }
}
fn storage_entry(binding:u32,read_only:bool)->wgpu::BindGroupLayoutEntry { wgpu::BindGroupLayoutEntry { binding,visibility:wgpu::ShaderStages::COMPUTE,ty:wgpu::BindingType::Buffer{ty:wgpu::BufferBindingType::Storage{read_only},has_dynamic_offset:false,min_binding_size:None},count:None } }
fn uniform_entry(binding:u32)->wgpu::BindGroupLayoutEntry { uniform_layout_entry(binding,wgpu::ShaderStages::COMPUTE) }
fn uniform_layout_entry(binding:u32,visibility:wgpu::ShaderStages)->wgpu::BindGroupLayoutEntry { wgpu::BindGroupLayoutEntry { binding,visibility,ty:wgpu::BindingType::Buffer{ty:wgpu::BufferBindingType::Uniform,has_dynamic_offset:false,min_binding_size:None},count:None } }
fn storage_texture_entry(binding:u32)->wgpu::BindGroupLayoutEntry { wgpu::BindGroupLayoutEntry { binding,visibility:wgpu::ShaderStages::COMPUTE,ty:wgpu::BindingType::StorageTexture{access:wgpu::StorageTextureAccess::WriteOnly,format:wgpu::TextureFormat::Rgba16Float,view_dimension:wgpu::TextureViewDimension::D2Array},count:None } }
fn bg_buffer<'a>(binding:u32,buffer:&'a wgpu::Buffer)->wgpu::BindGroupEntry<'a> { wgpu::BindGroupEntry{binding,resource:buffer.as_entire_binding()} }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authored_swell_size_is_independent_of_weather_wind() {
        let mut settings = OceanSettings::default();
        settings.wind.speed = 0.0;
        let calm = make_render_uniform(true, &settings);
        settings.wind.speed = 800.0;
        let windy = make_render_uniform(true, &settings);
        assert_eq!(calm.map_scales[..2], windy.map_scales[..2]);
        assert_eq!(calm.swell, windy.swell);
        assert_eq!(calm.map_scales[2][2], 0.0);
        assert!(windy.map_scales[2][2] > 0.0);
        settings.authored.amplitude *= 2.0;
        settings.authored.wavelength *= 2.0;
        let large = make_render_uniform(true, &settings);
        assert_eq!(large.map_scales[0][2], windy.map_scales[0][2] * 2.0);
        assert_eq!(large.map_scales[0][0], windy.map_scales[0][0] * 0.5);
        assert_eq!(large.map_scales[2], windy.map_scales[2]);
        settings.authored.slosh = 0.0;
        let primary = make_render_uniform(true, &settings);
        assert_eq!(primary.map_scales[0][2], 0.0);
        assert!(primary.swell[3] > 0.0);
    }

    #[test]
    fn clipmap_levels_nest_and_stay_in_bounds() {
        // A footprint wider than the outermost ring's reach needs the full stack.
        let clipmap = build_clipmap(OCEAN_HIGH_MESH_SPACING_UNITS, 100.0, [-100_000.0; 2], [100_000.0; 2], None);
        let side = (CLIPMAP_HALF_CELLS * 2 + 1) as usize;
        assert_eq!(clipmap.vertices.len(), side * side * CLIPMAP_LEVELS as usize);
        for index in &clipmap.indices {
            assert!((*index as usize) < clipmap.vertices.len());
        }
        assert_eq!(clipmap.indices.len() % 3, 0);

        // Level 0 is solid; every later level is an annulus with a quarter of
        // its cells removed, and that hole must land exactly on the previous
        // level's outer extent or the seam cannot close.
        let cells = (CLIPMAP_HALF_CELLS * 2) as usize;
        let hole = CLIPMAP_HALF_CELLS as usize;
        let expected = cells * cells + (CLIPMAP_LEVELS as usize - 1) * (cells * cells - hole * hole);
        assert_eq!(clipmap.indices.len(), expected * 6);

        for level in 1..CLIPMAP_LEVELS {
            let spacing = OCEAN_HIGH_MESH_SPACING_UNITS * (1u32 << level) as f32;
            let hole_extent = (CLIPMAP_HALF_CELLS / 2) as f32 * spacing;
            let inner_extent =
                CLIPMAP_HALF_CELLS as f32 * OCEAN_HIGH_MESH_SPACING_UNITS * (1u32 << (level - 1)) as f32;
            assert_eq!(hole_extent, inner_extent, "level {level} hole must match level {} extent", level - 1);
        }

        // Outer-boundary vertices carry the parent cell size so the shader can
        // snap them; interior vertices must not.
        let mut snapped = 0usize;
        for (index, vertex) in clipmap.vertices.iter().enumerate() {
            let within = index % (side * side);
            let column = (within % side) as i32 - CLIPMAP_HALF_CELLS;
            let row = (within / side) as i32 - CLIPMAP_HALF_CELLS;
            let outer = column.abs() == CLIPMAP_HALF_CELLS || row.abs() == CLIPMAP_HALF_CELLS;
            let level = index / (side * side);
            let expects_snap = outer && level + 1 < CLIPMAP_LEVELS as usize;
            assert_eq!(vertex.color[1] > 0.0, expects_snap);
            if expects_snap {
                assert_eq!(vertex.color[1], vertex.color[0] * 2.0);
                snapped += 1;
            }
        }
        assert!(snapped > 0);
    }

    fn reach(clipmap: &OceanClipmap) -> f32 {
        clipmap.vertices.iter()
            .map(|v| v.position[0].abs().max(v.position[2].abs()))
            .fold(0.0, f32::max)
    }

    #[test]
    fn clipmap_cell_size_does_not_depend_on_footprint() {
        let finest = |c: &OceanClipmap| c.vertices.iter().map(|v| v.color[0]).fold(f32::INFINITY, f32::min);
        let pool = build_clipmap(10.0, 0.0, [0.0, 0.0], [320.0, 384.0], None);
        let sea = build_clipmap(10.0, 0.0, [-1.0e5; 2], [1.0e5; 2], None);
        assert_eq!(finest(&pool), 10.0);
        assert_eq!(finest(&sea), 10.0);
        // The pool only drops rings it could never show.
        assert!(pool.indices.len() * 50 < sea.indices.len());
        assert_eq!(pool.coarsest_spacing, sea.coarsest_spacing);
    }

    #[test]
    fn clipmap_reaches_the_whole_footprint_from_any_centre() {
        for extent in [100.0f32, 320.0, 1408.0, 1500.0, 5000.0, 40_000.0] {
            let clipmap = build_clipmap(10.0, 0.0, [0.0; 2], [extent; 2], None);
            // The shader snaps the centre to color.b, so it can land half a
            // step outside the footprint.
            let step = clipmap.vertices[0].color[2];
            assert!(
                reach(&clipmap) >= extent + step * 0.5,
                "extent {extent}: reach {} step {step}", reach(&clipmap)
            );
            for index in &clipmap.indices {
                assert!((*index as usize) < clipmap.vertices.len());
            }
        }
    }

    #[test]
    fn contiguous_water_merges_and_biggest_comes_first() {
        let face = |plane: f32, x: [f32; 2], z: [f32; 2]| WaterFace {
            plane, minimum: [x[0], z[0]], maximum: [x[1], z[1]], authored_ocean: None,
        };
        let faces = [
            face(0.0, [0.0, 100.0], [0.0, 100.0]),       // small pool
            face(500.0, [0.0, 4000.0], [0.0, 4000.0]),   // sea, piece 1
            face(500.0, [4000.0, 9000.0], [0.0, 4000.0]),// sea, piece 2 (shares an edge)
            face(500.0, [0.0, 100.0], [9000.0, 9100.0]), // same plane, apart
            face(0.0, [90.0, 300.0], [0.0, 100.0]),      // overlaps the small pool
            face(0.4, [300.0, 400.0], [0.0, 100.0]),     // same plane within tolerance, touching
        ];
        let clusters = cluster_water_faces(&faces);
        assert_eq!(clusters.len(), 3);
        assert_eq!(clusters[0].members, vec![1, 2]);
        assert_eq!(clusters[0].minimum, [0.0, 0.0]);
        assert_eq!(clusters[0].maximum, [9000.0, 4000.0]);
        assert_eq!(clusters[1].members, vec![0, 4, 5]);
        assert_eq!(clusters[2].members, vec![3]);
        assert!(clusters.windows(2).all(|w| w[0].area() >= w[1].area()));

        // Different authored oceans, or different planes, never merge.
        let mut other = faces[2];
        other.authored_ocean = Some(0);
        assert_eq!(cluster_water_faces(&[faces[1], other]).len(), 2);
        assert_eq!(cluster_water_faces(&[faces[1], face(900.0, [0.0, 4000.0], [0.0, 4000.0])]).len(), 2);
    }

    #[test]
    fn mask_store_indexes_by_slot_and_refuses_overflow() {
        let tri: MaskTriangle = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]];
        let mut masks = OceanMasks::default();
        assert!(masks.push_surface(3, &[tri, tri]));
        assert!(masks.push_surface(0, &[tri]));
        assert_eq!(masks.ranges[3][..2], [0.0, 2.0]);
        assert_eq!(masks.ranges[0][..2], [2.0, 1.0]);
        assert_eq!(masks.triangles.len(), 6);
        assert!(!masks.push_surface(1, &[]), "empty outline means unmasked");
        assert!(!masks.push_surface(OCEAN_MAX_SURFACES, &[tri]));
        let too_many = vec![tri; OCEAN_MASK_VEC4S / 2];
        assert!(!masks.push_surface(2, &too_many), "overflow leaves the slot unmasked");
        assert_eq!(masks.ranges[2], [0.0; 4]);
    }

    #[test]
    fn clipmap_triangles_face_up() {
        let clipmap = build_clipmap(10.0, 0.0, [-1e9, -1e9], [1e9, 1e9], None);
        // The renderer culls back faces with a counter-clockwise front face, so
        // the clipmap must wind the same way as an authored +Y water face.
        for triangle in clipmap.indices.chunks_exact(3).take(4096) {
            let p: Vec<[f32; 3]> = triangle.iter().map(|i| clipmap.vertices[*i as usize].position).collect();
            let u = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
            let v = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
            let normal_y = u[2] * v[0] - u[0] * v[2];
            assert!(normal_y > 0.0, "clipmap triangle winds downward");
        }
    }
}

//! Water mesh for a sparse river beyond the camera window: the UE 5.8.1 Water
//! plugin's CPU water-mesh quadtree (river_quadtree.rs) over the simulation's
//! wet tiles, traversed per view into per-density instance buckets, drawn as
//! flat tiles by the water vertex factory (river_far.wgsl) with the river WPO
//! from the sparse pool. Copyright Epic Games, Inc. All Rights Reserved for the
//! ported algorithms.
//!
//! Configuration (WaterMeshSceneProxy constructor, WaterMeshComponent):
//! - LeafSize = TileSize = one simulation tile (tile cells * dx);
//! - TessellationFactor = log2(tile cells): LOD0 quads match the sim cells;
//! - LODScale = LeafSize * max(component LODScale 1, 0.5);
//! - DensityCount = min(TreeDepth, floor(log2(NumQuadsLOD0))), one vertex
//!   factory per density with NumQuads halving;
//! - ForceCollapseDensityLevel default (never), LOD morphing on
//!   (r.Water.WaterMesh.LODMorphEnabled 1).
//! The tree is rebuilt (InitTree, AddWaterTilesInsideBounds per wet tile,
//! Unlock with pruning) when the wet tile set or the camera window changes;
//! Unreal rebuilds when water bodies change. Tiles wholly inside the window's
//! interior are left out, and the far pixel shader discards that interior.
use crate::river_quadtree::{tile_bounds, TraversalDesc, TraversalOutput, WaterQuadTree};
use jka_water::river::CENTIMETRES_PER_MAP_UNIT;
use std::collections::{BTreeSet, HashMap};
use wgpu::util::DeviceExt;
use std::sync::atomic::{AtomicBool, Ordering};

// Session cvars: r_riverWaterMesh (draw the quadtree tiles beyond the camera
// window) and r_riverLodMorph (r.Water.WaterMesh.LODMorphEnabled).
static ENABLED: AtomicBool = AtomicBool::new(true);
static LOD_MORPH: AtomicBool = AtomicBool::new(true);

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
}
pub fn lod_morph() -> bool {
    LOD_MORPH.load(Ordering::Relaxed)
}
pub fn set_lod_morph(enabled: bool) {
    LOD_MORPH.store(enabled, Ordering::Relaxed);
}

/// `RiverFar` in river_far.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct FarUniform {
    origin: [f32; 4],
    lattice: [i32; 4],
    lod: [f32; 4],
    window: [f32; 4],
}

/// `RiverFarInstance` in river_far.wgsl (80 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct FarInstance {
    translation: [f32; 4],
    packed: [u32; 4],
    scale: [f32; 4],
    ambient: [f32; 4],
    directed: [f32; 4],
}

/// One `FWaterVertexFactory` density: FWaterMeshVertexBuffer positions in
/// [-0.5, 0.5] and the Morton-ordered FWaterMeshIndexBuffer.
pub struct DensityMesh {
    pub vertices: wgpu::Buffer,
    pub indices: wgpu::Buffer,
    pub index_count: u32,
}

/// `FMath::ReverseMortonCode2`: the even bits of `x`, compacted.
fn reverse_morton_code2(x: u32) -> u32 {
    let mut x = x & 0x5555_5555;
    x = (x ^ (x >> 1)) & 0x3333_3333;
    x = (x ^ (x >> 2)) & 0x0f0f_0f0f;
    x = (x ^ (x >> 4)) & 0x00ff_00ff;
    x = (x ^ (x >> 8)) & 0x0000_ffff;
    x
}

/// FWaterMeshVertexBuffer + FWaterMeshIndexBuffer for `quads` per side.
pub fn tile_mesh(quads: u32) -> (Vec<[f32; 2]>, Vec<u32>) {
    let side = quads + 1;
    let mut vertices = Vec::with_capacity((side * side) as usize);
    for y in 0..side {
        for x in 0..side {
            vertices.push([x as f32 / quads as f32 - 0.5, y as f32 / quads as f32 - 0.5]);
        }
    }
    let mut indices = Vec::with_capacity((quads * quads * 6) as usize);
    for morton in 0..quads * quads {
        let square_x = reverse_morton_code2(morton);
        let square_y = reverse_morton_code2(morton >> 1);
        let forward_diagonal = (square_x % 2 == 1) != (square_y % 2 == 1);
        let i0 = square_x + square_y * side;
        let i1 = i0 + 1;
        let i2 = i0 + side;
        let i3 = i2 + 1;
        indices.extend([i3, i1, if forward_diagonal { i2 } else { i0 }, i0, i2, if forward_diagonal { i1 } else { i3 }]);
    }
    (vertices, indices)
}

pub fn vertex_layout() -> wgpu::VertexBufferLayout<'static> {
    const ATTRIBUTES: [wgpu::VertexAttribute; 1] = [wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 }];
    wgpu::VertexBufferLayout { array_stride: 8, step_mode: wgpu::VertexStepMode::Vertex, attributes: &ATTRIBUTES }
}

/// What the far mesh needs from the sparse river each frame.
pub struct FarInputs<'a> {
    pub wet_tiles: &'a BTreeSet<[i32; 2]>,
    pub wet_generation: u64,
    pub pool_generation: u64,
    pub table: &'a wgpu::Buffer,
    pub surface: &'a wgpu::Buffer,
    pub normals: &'a wgpu::Buffer,
    pub lattice: jka_water::river::sparse::Lattice,
    pub dx_cm: f32,
    /// Lattice corner XY and river Z, cm.
    pub solver_origin_cm: [f64; 3],
    /// Camera window: first cell and cells per side, if placed.
    pub window: Option<([i32; 2], u32)>,
    /// Lowest bed candidate per tile range, map units (Bins::min_z).
    pub tile_min_z: &'a dyn Fn([i32; 2]) -> Option<f32>,
    /// The source water level, map units (the highest the river can stand).
    pub water_level: f32,
}

/// Per-view inputs.
pub struct FarView<'a> {
    /// Eye in river cm (JKA axes).
    pub eye_cm: [f64; 3],
    /// `Frustum.IntersectBox` on a river-cm box (centre, extent).
    pub frustum: &'a dyn Fn([f64; 3], [f64; 3]) -> bool,
    pub lod_morphing: bool,
}

pub struct FarMesh {
    densities: Vec<DensityMesh>,
    num_quads_lod0: u32,
    density_count: i32,
    uniform: wgpu::Buffer,
    instances: wgpu::Buffer,
    instance_capacity: u64,
    tree: Option<WaterQuadTree>,
    tree_key: Option<(u64, Option<([i32; 2], u32)>)>,
    leaf_size: f64,
    lod_scale: f64,
    output: TraversalOutput,
    /// (first instance, count) per density bucket for this frame's draws.
    pub draws: Vec<(u32, u32)>,
    group: Option<(u64, wgpu::BindGroup)>,
    light_cache: HashMap<[i64; 2], ([f32; 4], [f32; 4])>,
    pub stats: FarStats,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FarStats {
    pub leaves: usize,
    pub nodes: usize,
    pub tree_depth: i32,
    pub instances: i32,
    pub rebuilds: u64,
}

impl FarMesh {
    pub fn new(device: &wgpu::Device, tile_cells: u32) -> Self {
        let num_quads_lod0 = tile_cells.max(2);
        // One vertex factory per density, NumQuads halving until 1.
        let mut densities = Vec::new();
        let mut quads = num_quads_lod0;
        while quads > 1 {
            let (vertices, indices) = tile_mesh(quads);
            densities.push(DensityMesh {
                vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("river water mesh tile vertices"),
                    contents: bytemuck::cast_slice(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
                indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("river water mesh tile indices"),
                    contents: bytemuck::cast_slice(&indices),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                index_count: indices.len() as u32,
            });
            quads /= 2;
        }
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("river water mesh"),
            size: std::mem::size_of::<FarUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instance_capacity = 256;
        Self {
            densities,
            num_quads_lod0,
            density_count: 0,
            uniform,
            instances: Self::instance_buffer(device, instance_capacity),
            instance_capacity,
            tree: None,
            tree_key: None,
            leaf_size: 0.0,
            lod_scale: 0.0,
            output: TraversalOutput::default(),
            draws: Vec::new(),
            group: None,
            light_cache: HashMap::new(),
            stats: FarStats::default(),
        }
    }

    fn instance_buffer(device: &wgpu::Device, capacity: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("river water mesh instances"),
            size: capacity * std::mem::size_of::<FarInstance>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    pub fn densities(&self) -> &[DensityMesh] {
        &self.densities
    }

    pub fn group(&self) -> Option<&wgpu::BindGroup> {
        self.group.as_ref().map(|(_, g)| g)
    }

    /// Rebuild the quadtree when the wet tiles or the window changed.
    fn rebuild(&mut self, inputs: &FarInputs<'_>) {
        let key = (inputs.wet_generation, inputs.window);
        if self.tree_key == Some(key) && self.tree.is_some() {
            return;
        }
        self.tree_key = Some(key);
        self.light_cache.clear();
        let lattice = inputs.lattice;
        let tile = lattice.tile();
        let tiles = lattice.tiles();
        let dx = f64::from(inputs.dx_cm);
        self.leaf_size = f64::from(tile) * dx;
        // LODScale = LeafSize * Max(LODScale 1.0, 0.5).
        self.lod_scale = self.leaf_size * 1.0f64.max(0.5);
        let origin = inputs.solver_origin_cm;
        let bounds_min = [origin[0], origin[1]];
        let bounds_max = [origin[0] + f64::from(tiles[0]) * self.leaf_size, origin[1] + f64::from(tiles[1]) * self.leaf_size];
        let extent = [((tiles[0] + 1) / 2).max(1) as i32, ((tiles[1] + 1) / 2).max(1) as i32];
        let mut tree = WaterQuadTree::init_tree(bounds_min, bounds_max, self.leaf_size, extent);
        // Window tiles whose every cell is in the window's drawn interior.
        let inner = inputs.window.map(|(first, cells)| {
            let t = tile as i32;
            let first_tile = [first[0].div_euclid(t), first[1].div_euclid(t)];
            let n = (cells / tile) as i32;
            [first_tile[0] + 1, first_tile[1] + 1, first_tile[0] + n - 2, first_tile[1] + n - 2]
        });
        let cm = f64::from(CENTIMETRES_PER_MAP_UNIT);
        let level_cm = f64::from(inputs.water_level) * cm;
        let mut leaves = 0;
        for &t in inputs.wet_tiles {
            if inner.is_some_and(|r| t[0] >= r[0] && t[1] >= r[1] && t[0] <= r[2] && t[1] <= r[3]) {
                continue;
            }
            let bed = (inputs.tile_min_z)(t).map_or(level_cm, |z| f64::from(z) * cm);
            // Z bounds: the bed up to the source level (river base height is
            // Bounds.Max.Z); the WPO lifts from the solver origin.
            let z = [bed.min(level_cm), level_cm];
            let min_xy = [origin[0] + f64::from(t[0]) * self.leaf_size, origin[1] + f64::from(t[1]) * self.leaf_size];
            tree.add_water_tiles_inside_bounds(tile_bounds(min_xy, self.leaf_size, z, 0.01 * dx));
            leaves += 1;
        }
        tree.unlock(true);
        self.density_count = tree.tree_depth().min(self.num_quads_lod0.ilog2() as i32).min(self.densities.len() as i32);
        self.stats.leaves = leaves;
        self.stats.nodes = tree.node_count();
        self.stats.tree_depth = tree.tree_depth();
        self.stats.rebuilds += 1;
        self.tree = Some(tree);
    }

    /// Rebuild if needed, traverse for this view, upload the instances and
    /// the uniform, and (re)bind the pool. `light` samples the classic
    /// indirect field at a map point (river_slw::classic_ambient_texels).
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        authored: &wgpu::Buffer,
        inputs: &FarInputs<'_>,
        view: &FarView<'_>,
        light: &mut dyn FnMut([f32; 3]) -> Option<jka_render_core::light::ClassicEntityLight>,
        map_toward_sun: Option<[f32; 3]>,
    ) {
        self.draws.clear();
        self.rebuild(inputs);
        let Some(tree) = &self.tree else { return };
        if self.density_count <= 0 || tree.node_count() == 0 || self.stats.leaves == 0 {
            self.stats.instances = 0;
            return;
        }
        // GetWaterLODParams, then the traversal description.
        let lod = tree.water_lod_params(view.eye_cm, self.lod_scale);
        let desc = TraversalDesc {
            lowest_lod: lod.lowest_lod,
            lod_count: tree.tree_depth(),
            density_count: self.density_count,
            height_morph: lod.height_lod_factor,
            force_collapse_density_level: i32::MAX,
            lod_scale: self.lod_scale,
            observer_position: view.eye_cm,
            frustum: view.frustum,
            lod_morphing_enabled: view.lod_morphing,
        };
        tree.build_water_tile_instance_data(&desc, &mut self.output);
        self.stats.instances = self.output.instance_count;
        if self.output.instance_count == 0 {
            return;
        }
        // Bucketed (by density) instance array; light per tile centre.
        let mut order: Vec<usize> = (0..self.output.staging_instance_data.len()).collect();
        order.sort_by_key(|&i| self.output.staging_instance_data[i].bucket_index);
        let cm = f64::from(CENTIMETRES_PER_MAP_UNIT);
        let mut instances = Vec::with_capacity(order.len());
        for &i in &order {
            let s = self.output.staging_instance_data[i];
            let key = [(s.translation[0] / self.leaf_size * 4.0).round() as i64, (s.translation[1] / self.leaf_size * 4.0).round() as i64];
            let (ambient, directed) = *self.light_cache.entry(key).or_insert_with(|| {
                let point = [(s.translation[0] / cm) as f32, (s.translation[1] / cm) as f32, inputs.water_level];
                let texels = crate::river_slw::classic_ambient_texels([1, 1], |_, _| point, &mut *light, map_toward_sun);
                (texels.ambient[0], texels.directed[0])
            });
            let packed = (s.lod_level & 0xFF) | (u32::from(s.should_morph) << 8) | (u32::from(s.can_morph_twice) << 9) | ((s.density_index & 0x1F) << 10);
            instances.push(FarInstance {
                translation: [s.translation[0] as f32, s.translation[1] as f32, s.translation[2] as f32, 0.0],
                packed: [packed, 0, 0, 0],
                scale: [s.scale[0] as f32, s.scale[1] as f32, s.height_morph, 0.0],
                ambient,
                directed,
            });
        }
        let mut first = 0u32;
        for &count in &self.output.bucket_instance_counts {
            self.draws.push((first, count as u32));
            first += count as u32;
        }
        if instances.len() as u64 > self.instance_capacity {
            self.instance_capacity = (instances.len() as u64).next_power_of_two();
            self.instances = Self::instance_buffer(device, self.instance_capacity);
            self.group = None;
        }
        queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&instances));
        let lattice = inputs.lattice;
        let origin = inputs.solver_origin_cm;
        let dx = f64::from(inputs.dx_cm);
        let window = inputs.window.map_or([1.0, 1.0, -1.0, -1.0], |(first, cells)| {
            let lo = [origin[0] + (f64::from(first[0]) + 1.0) * dx, origin[1] + (f64::from(first[1]) + 1.0) * dx];
            let hi = [origin[0] + (f64::from(first[0]) + f64::from(cells) - 1.0) * dx, origin[1] + (f64::from(first[1]) + f64::from(cells) - 1.0) * dx];
            [lo[0] as f32, lo[1] as f32, hi[0] as f32, hi[1] as f32]
        });
        let uniform = FarUniform {
            origin: [origin[0] as f32, origin[1] as f32, origin[2] as f32, inputs.dx_cm],
            lattice: [lattice.dimensions()[0] as i32, lattice.dimensions()[1] as i32, lattice.tiles()[0] as i32, lattice.shift() as i32],
            lod: [self.lod_scale as f32, if view.lod_morphing { 1.0 } else { 0.0 }, self.num_quads_lod0 as f32, lattice.tile() as f32],
            window,
        };
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        if self.group.as_ref().is_none_or(|(g, _)| *g != inputs.pool_generation) {
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("river water mesh"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 9, resource: authored.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 10, resource: self.uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 11, resource: inputs.table.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 12, resource: inputs.surface.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 13, resource: inputs.normals.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 14, resource: self.instances.as_entire_binding() },
                ],
            });
            self.group = Some((inputs.pool_generation, group));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_mesh_matches_upstream_layout() {
        let (v, i) = tile_mesh(4);
        assert_eq!(v.len(), 25);
        assert_eq!(v[0], [-0.5, -0.5]);
        assert_eq!(v[24], [0.5, 0.5]);
        assert_eq!(i.len(), 4 * 4 * 6);
        // Morton 0: square (0,0), not forward diagonal: 3,1,0, 0,2,3 (side 5).
        assert_eq!(&i[..6], &[6, 1, 0, 0, 5, 6]);
        // Morton 1: square (1,0): forward diagonal.
        assert_eq!(&i[6..12], &[7, 2, 6, 1, 6, 2]);
        assert!(i.iter().all(|&x| x < 25));
    }

    /// The whole river surface module (window + water mesh entry points)
    /// composes and validates; catches WGSL reserved words and type errors.
    #[test]
    fn river_surface_module_with_water_mesh_validates() {
        let module = crate::river_surface::compose().expect("river surface shader composes");
        wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("river surface shader validates: {e:?}"));
        for entry in ["vs", "fs", "fs_composite", "vs_far", "fs_far", "fs_composite_far"] {
            assert!(module.entry_points.iter().any(|e| e.name == entry), "missing entry point {entry}");
        }
    }

    #[test]
    fn reverse_morton_matches_definition() {
        assert_eq!(reverse_morton_code2(0b0000), 0);
        assert_eq!(reverse_morton_code2(0b0001), 1);
        assert_eq!(reverse_morton_code2(0b0100), 2);
        assert_eq!(reverse_morton_code2(0b0101), 3);
        assert_eq!(reverse_morton_code2(0b0010), 0);
    }
}

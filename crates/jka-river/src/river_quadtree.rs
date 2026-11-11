//! Port of the UE 5.8.1 Water plugin's water mesh quadtree
//! (Plugins/Experimental/Water/Source/Runtime: WaterQuadTree.h/.cpp and the
//! LOD parameters of WaterMeshSceneProxy.cpp `GetWaterLODParams`).
//! Original algorithms: Copyright Epic Games, Inc. All Rights Reserved.
//!
//! The CPU quadtree path (not the GPU quadtree): nodes are inserted with
//! `AddWaterTilesInsideBounds`, pruned by `Unlock`, and traversed per view by
//! `BuildWaterTileInstanceData` -> `SelectLOD` / `SelectLODRefinement`.
//!
//! What is used here and what is left out:
//! - One water body: a river (`IsRiver`: base height = node `Bounds.Max.Z`).
//!   Water-body render data slot 0 is the native "null" entry with the lowest
//!   priority and no material; slot 1 is the river.
//! - River-to-lake/ocean transitions, oceans/lakes (polygon insertion), the
//!   far mesh, occlusion culling (and so breadth-first indices), hit proxies
//!   and debug drawing are not ported: a sparse simulation river has none.
//! - Leaves are the simulation's wet tiles. Unreal inserts water-body bounds,
//!   and `FBox::IntersectXY` is inclusive, so a box exactly one tile wide
//!   would also create its edge neighbours. Callers inset tile boxes by a small
//!   epsilon (see `tile_bounds`), which selects exactly that leaf.
//! Units are whatever the caller uses consistently (river centimetres here).

pub const INVALID_PARENT: u32 = 0x0FFF_FFFF;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Box3 {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Box3 {
    pub fn new(min: [f64; 3], max: [f64; 3]) -> Self {
        Self { min, max }
    }
    pub fn center(&self) -> [f64; 3] {
        std::array::from_fn(|a| (self.min[a] + self.max[a]) * 0.5)
    }
    pub fn extent(&self) -> [f64; 3] {
        std::array::from_fn(|a| (self.max[a] - self.min[a]) * 0.5)
    }
    pub fn size(&self) -> [f64; 3] {
        std::array::from_fn(|a| self.max[a] - self.min[a])
    }
    /// `FBox::IntersectXY` (inclusive).
    pub fn intersect_xy(&self, other: &Box3) -> bool {
        !(self.min[0] > other.max[0] || other.min[0] > self.max[0] || self.min[1] > other.max[1] || other.min[1] > self.max[1])
    }
    /// `FBox2D::ComputeSquaredDistanceToPoint` of the XY footprint.
    pub fn squared_distance_xy(&self, point: [f64; 2]) -> f64 {
        let mut d = 0.0;
        for a in 0..2 {
            if point[a] < self.min[a] {
                d += (point[a] - self.min[a]).powi(2);
            } else if point[a] > self.max[a] {
                d += (point[a] - self.max[a]).powi(2);
            }
        }
        d
    }
}

/// `FWaterBodyRenderData`, reduced to what a single river reads.
#[derive(Clone, Copy, Debug)]
struct WaterBodyRenderData {
    has_material: bool,
    priority: i16,
}

#[derive(Clone, Copy, Debug)]
struct Node {
    bounds: Box3,
    water_body_index: u16,
    parent_index: u32,
    has_complete_subtree: bool,
    is_subtree_same_water_body: bool,
    has_material: bool,
    children: [u32; 4],
}

impl Default for Node {
    fn default() -> Self {
        Self {
            bounds: Box3::new([-1.0; 3], [1.0; 3]),
            water_body_index: 0,
            parent_index: INVALID_PARENT,
            has_complete_subtree: true,
            is_subtree_same_water_body: true,
            has_material: false,
            children: [0; 4],
        }
    }
}

/// `FWaterQuadTree::FTraversalDesc` (occlusion and debug members omitted).
pub struct TraversalDesc<'a> {
    pub lowest_lod: i32,
    pub lod_count: i32,
    pub density_count: i32,
    pub height_morph: f32,
    pub force_collapse_density_level: i32,
    pub lod_scale: f64,
    pub observer_position: [f64; 3],
    /// `Frustum.IntersectBox(Center, Extent)`.
    pub frustum: &'a dyn Fn([f64; 3], [f64; 3]) -> bool,
    pub lod_morphing_enabled: bool,
}

/// One `FStagingInstanceData`: Data[0] (xyz translate) and Data[1] (packed LOD
/// level / bShouldMorph / bCanMorphTwice, HeightMorph, scale XY).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StagingInstance {
    pub bucket_index: usize,
    pub translation: [f64; 3],
    pub lod_level: u32,
    pub should_morph: bool,
    pub can_morph_twice: bool,
    pub height_morph: f32,
    pub scale: [f64; 2],
    /// The density index of the bucket (one material: bucket == density).
    pub density_index: u32,
}

#[derive(Default, Debug)]
pub struct TraversalOutput {
    pub bucket_instance_counts: Vec<i32>,
    pub staging_instance_data: Vec<StagingInstance>,
    pub instance_count: i32,
}

/// `FViewWaterQuadTree::FWaterLODParams`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterLodParams {
    pub lowest_lod: i32,
    pub height_lod_factor: f32,
    pub water_height_for_lod: f32,
}

pub struct WaterQuadTree {
    tree_depth: i32,
    leaf_size: f64,
    max_leaf_count: i32,
    extent_in_tiles: [i32; 2],
    tile_region_min: [f64; 2],
    tile_region_max: [f64; 2],
    nodes: Vec<Node>,
    water_body_render_data: Vec<WaterBodyRenderData>,
    read_only: bool,
}

/// The river's index in the water-body render data (slot 0 is the null entry).
pub const RIVER_WATER_BODY: u32 = 1;

impl WaterQuadTree {
    /// `InitTree` plus `AddWaterBodyRenderData` for the one river.
    pub fn init_tree(bounds_min: [f64; 2], bounds_max: [f64; 2], tile_size: f64, extent_in_tiles: [i32; 2]) -> Self {
        assert!(tile_size > 0.0 && extent_in_tiles.iter().all(|&e| e > 0));
        let max_dim = (extent_in_tiles[0] * 2).max(extent_in_tiles[1] * 2);
        let root_dim = (max_dim as u32).next_power_of_two() as f64;
        let capacity = ((root_dim * root_dim * 4.0) / 3.0) as usize;
        let mut nodes = Vec::with_capacity(capacity.max(1));
        let root_world_size = root_dim * tile_size;
        let mut root = Node::default();
        root.bounds = Box3::new(
            [bounds_min[0], bounds_min[1], f64::from(f32::MAX)],
            [bounds_min[0] + root_world_size, bounds_min[1] + root_world_size, f64::from(f32::MIN)],
        );
        nodes.push(root);
        Self {
            tree_depth: root_dim.log2() as i32,
            leaf_size: tile_size,
            max_leaf_count: extent_in_tiles[0] * extent_in_tiles[1] * 4,
            extent_in_tiles,
            tile_region_min: bounds_min,
            tile_region_max: bounds_max,
            nodes,
            // Slot 0: the defaulted "null" render data (no material, lowest
            // priority); slot 1: the river.
            water_body_render_data: vec![
                WaterBodyRenderData { has_material: false, priority: i16::MIN },
                WaterBodyRenderData { has_material: true, priority: 0 },
            ],
            read_only: false,
        }
    }

    pub fn tree_depth(&self) -> i32 {
        self.tree_depth
    }
    pub fn leaf_size(&self) -> f64 {
        self.leaf_size
    }
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
    pub fn max_leaf_count(&self) -> i32 {
        self.max_leaf_count
    }
    pub fn resolution(&self) -> [i32; 2] {
        self.extent_in_tiles.map(|e| e * 2)
    }

    /// `GetLODDistance`.
    pub fn lod_distance(lod_level: i32, lod_scale: f64) -> f64 {
        2.0f64.powf(f64::from(lod_level + 1)) * lod_scale
    }

    /// `AddWaterTilesInsideBounds` for the river.
    pub fn add_water_tiles_inside_bounds(&mut self, bounds: Box3) {
        assert!(!self.read_only);
        let mesh_bounds = Box3::new([self.tile_region_min[0], self.tile_region_min[1], 0.0], [self.tile_region_max[0], self.tile_region_max[1], 0.0]);
        self.add_nodes(0, &mesh_bounds, &bounds, RIVER_WATER_BODY, self.tree_depth, 0);
    }

    /// `FNode::AddNodes`.
    fn add_nodes(&mut self, node: usize, mesh_bounds: &Box3, water_body_bounds: &Box3, water_body_index: u32, lod_level: i32, parent_index: u32) {
        let incoming = self.water_body_render_data[water_body_index as usize];
        let this = self.water_body_render_data[self.nodes[node].water_body_index as usize];
        {
            let n = &mut self.nodes[node];
            n.bounds.max[2] = n.bounds.max[2].max(water_body_bounds.max[2]);
            n.bounds.min[2] = n.bounds.min[2].min(water_body_bounds.min[2]);
            // (River-to-lake/ocean transition marking: no other water body types.)
            if incoming.priority >= this.priority {
                n.water_body_index = water_body_index as u16;
                n.has_material = self.water_body_render_data[water_body_index as usize].has_material;
            }
            n.is_subtree_same_water_body = true;
            n.has_complete_subtree = true;
        }
        if lod_level == 0 {
            return;
        }
        let bounds = self.nodes[node].bounds;
        let half = [(bounds.max[0] - bounds.min[0]) * 0.5, (bounds.max[1] - bounds.min[1]) * 0.5];
        let mut prev_child = self.nodes[0];
        const HALF_OFFSETS: [[f64; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
        for i in 0..4 {
            let child = self.nodes[node].children[i];
            if child > 0 {
                if self.nodes[child as usize].bounds.intersect_xy(water_body_bounds) {
                    self.add_nodes(child as usize, mesh_bounds, water_body_bounds, water_body_index, lod_level - 1, child);
                }
            } else {
                let child_min = [bounds.min[0] + half[0] * HALF_OFFSETS[i][0], bounds.min[1] + half[1] * HALF_OFFSETS[i][1], water_body_bounds.min[2]];
                let child_max = [child_min[0] + half[0], child_min[1] + half[1], water_body_bounds.max[2]];
                let child_bounds = Box3::new(child_min, child_max);
                if child_bounds.intersect_xy(water_body_bounds) && child_bounds.intersect_xy(mesh_bounds) {
                    let index = self.nodes.len() as u32;
                    self.nodes.push(Node { bounds: child_bounds, parent_index, ..Node::default() });
                    self.nodes[node].children[i] = index;
                    self.add_nodes(index as usize, mesh_bounds, water_body_bounds, water_body_index, lod_level - 1, index);
                }
            }
            let child = self.nodes[node].children[i];
            if child > 0 {
                let child_node = self.nodes[child as usize];
                if prev_child.parent_index == INVALID_PARENT {
                    prev_child = child_node;
                }
                // CanMerge: same water body (no transitions here).
                if !child_node.is_subtree_same_water_body || child_node.water_body_index != prev_child.water_body_index {
                    self.nodes[node].is_subtree_same_water_body = false;
                }
                prev_child = child_node;
                if !child_node.has_complete_subtree {
                    self.nodes[node].has_complete_subtree = false;
                }
            } else {
                self.nodes[node].has_complete_subtree = false;
            }
        }
    }

    /// `Unlock`: prune redundant nodes (breadth-first indices are only used
    /// by occlusion culling, which is not ported).
    pub fn unlock(&mut self, prune_redundant_nodes: bool) {
        if prune_redundant_nodes {
            let mut end_index = self.nodes.len() as i64 - 1;
            let mut node_index = end_index;
            while node_index > 0 {
                let ni = node_index as usize;
                let parent = self.nodes[ni].parent_index as usize;
                let parent_node = self.nodes[parent];
                let node = self.nodes[ni];
                if parent_node.has_complete_subtree && parent_node.is_subtree_same_water_body {
                    self.nodes[parent].children = [0; 4];
                    self.swap_remove(ni, end_index as usize);
                    end_index -= 1;
                } else if !node.has_material && node.has_complete_subtree && node.is_subtree_same_water_body {
                    for c in &mut self.nodes[parent].children {
                        if *c == ni as u32 {
                            *c = 0;
                        }
                    }
                    self.swap_remove(ni, end_index as usize);
                    end_index -= 1;
                }
                node_index -= 1;
            }
            self.nodes.truncate((end_index + 1) as usize);
        }
        self.read_only = true;
    }

    fn swap_remove(&mut self, node_index: usize, end_index: usize) {
        if node_index == end_index {
            return;
        }
        self.nodes.swap(node_index, end_index);
        let moved = self.nodes[node_index];
        for i in 0..4 {
            if moved.children[i] > 0 {
                self.nodes[moved.children[i] as usize].parent_index = node_index as u32;
            }
            let parent = &mut self.nodes[moved.parent_index as usize];
            if parent.children[i] == end_index as u32 {
                parent.children[i] = node_index as u32;
            }
        }
    }

    /// `BuildWaterTileInstanceData` (no far mesh).
    pub fn build_water_tile_instance_data(&self, desc: &TraversalDesc<'_>, output: &mut TraversalOutput) {
        assert!(self.read_only);
        output.bucket_instance_counts = vec![0; desc.density_count.max(1) as usize];
        output.staging_instance_data.clear();
        output.instance_count = 0;
        if self.nodes.is_empty() {
            return;
        }
        let root = self.nodes[0];
        self.select_lod(&root, self.tree_depth, desc, output);
    }

    /// `FNode::CanRender`.
    fn can_render(&self, node: &Node, density_level: i32, force_collapse_density_level: i32) -> bool {
        self.water_body_render_data[node.water_body_index as usize].has_material
            && node.is_subtree_same_water_body
            && (density_level > force_collapse_density_level || node.has_complete_subtree)
    }

    /// `FNode::AddNodeForRender` (river: base height = Bounds.Max.Z).
    fn add_node_for_render(&self, node: &Node, density_level: i32, lod_level: i32, desc: &TraversalDesc<'_>, output: &mut TraversalOutput) {
        let base_height = node.bounds.max[2];
        let density_index = density_level.min(desc.density_count - 1);
        let bucket_index = density_index as usize; // MaterialIndex 0 * DensityCount + DensityIndex
        output.bucket_instance_counts[bucket_index] += 1;
        let center = node.bounds.center();
        let size = node.bounds.size();
        let is_lowest_lod = lod_level == desc.lowest_lod;
        output.staging_instance_data.push(StagingInstance {
            bucket_index,
            translation: [center[0], center[1], base_height],
            lod_level: (lod_level & 0xFF) as u32,
            should_morph: desc.lod_morphing_enabled && density_index != desc.density_count - 1,
            can_morph_twice: density_index < desc.density_count - 2,
            height_morph: if is_lowest_lod { desc.height_morph } else { 0.0 },
            scale: [size[0], size[1]],
            density_index: density_index as u32,
        });
        output.instance_count += 1;
    }

    fn visible(node: &Node, desc: &TraversalDesc<'_>) -> bool {
        // WaterInfoBounds: the whole lattice is the water zone here.
        (desc.frustum)(node.bounds.center(), node.bounds.extent())
    }

    /// `FNode::SelectLODRefinement`.
    fn select_lod_refinement(&self, node: &Node, density_level: i32, lod_level: i32, desc: &TraversalDesc<'_>, output: &mut TraversalOutput) {
        if !Self::visible(node, desc) {
            return;
        }
        if self.can_render(node, density_level, desc.force_collapse_density_level) {
            self.add_node_for_render(node, density_level, lod_level, desc, output);
        } else {
            for &child in &node.children {
                if child > 0 {
                    self.select_lod_refinement(&self.nodes[child as usize], density_level + 1, lod_level, desc, output);
                }
            }
        }
    }

    /// `FNode::SelectLOD`.
    fn select_lod(&self, node: &Node, lod_level: i32, desc: &TraversalDesc<'_>, output: &mut TraversalOutput) {
        if !Self::visible(node, desc) {
            return;
        }
        let closest = node.bounds.squared_distance_xy([desc.observer_position[0], desc.observer_position[1]]).sqrt();
        if closest > Self::lod_distance(lod_level, desc.lod_scale) {
            if self.can_render(node, 0, desc.force_collapse_density_level) {
                self.add_node_for_render(node, 1, lod_level + 1, desc, output);
            } else {
                for &child in &node.children {
                    if child > 0 {
                        self.select_lod_refinement(&self.nodes[child as usize], 2, lod_level + 1, desc, output);
                    }
                }
            }
            return;
        }
        if lod_level == 0 {
            if self.can_render(node, 0, desc.force_collapse_density_level) {
                self.add_node_for_render(node, 0, lod_level, desc, output);
            }
        } else if closest > Self::lod_distance(lod_level - 1, desc.lod_scale) || lod_level == desc.lowest_lod {
            if self.can_render(node, 0, desc.force_collapse_density_level) {
                self.add_node_for_render(node, 0, lod_level, desc, output);
            } else {
                for &child in &node.children {
                    if child > 0 {
                        self.select_lod_refinement(&self.nodes[child as usize], 1, lod_level, desc, output);
                    }
                }
            }
        } else if node.has_complete_subtree && node.is_subtree_same_water_body {
            // Implicit children of a complete subtree.
            let half = [node.bounds.extent()[0], node.bounds.extent()[1], node.bounds.extent()[2] * 2.0];
            const HALF_OFFSETS: [[f64; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]];
            for offset in HALF_OFFSETS {
                let min = [node.bounds.min[0] + half[0] * offset[0], node.bounds.min[1] + half[1] * offset[1], node.bounds.min[2]];
                let max = [min[0] + half[0], min[1] + half[1], min[2] + half[2]];
                let child = Node {
                    bounds: Box3::new(min, max),
                    water_body_index: node.water_body_index,
                    has_complete_subtree: true,
                    is_subtree_same_water_body: true,
                    ..Node::default()
                };
                self.select_lod(&child, lod_level - 1, desc, output);
            }
        } else {
            for &child in &node.children {
                if child > 0 {
                    self.select_lod(&self.nodes[child as usize], lod_level - 1, desc, output);
                }
            }
        }
    }

    /// `FNode::QueryBaseHeightAtLocation`.
    fn query_base_height(&self, node: &Node, xy: [f64; 2]) -> (bool, f64) {
        if node.has_complete_subtree && node.is_subtree_same_water_body {
            return (true, node.bounds.max[2]);
        }
        for &child in &node.children {
            if child > 0 {
                let b = self.nodes[child as usize].bounds;
                if xy[0] >= b.min[0] && xy[0] < b.max[0] && xy[1] >= b.min[1] && xy[1] < b.max[1] {
                    return self.query_base_height(&self.nodes[child as usize], xy);
                }
            }
        }
        // SurfaceBaseHeight of a river's render data is not used for rivers; 0.
        (false, 0.0)
    }

    /// `QueryTileBaseHeightAtLocation`.
    pub fn query_tile_base_height_at_location(&self, xy: [f64; 2]) -> (bool, f64) {
        if self.nodes.is_empty() {
            return (false, 0.0);
        }
        self.query_base_height(&self.nodes[0], xy)
    }

    /// `QueryInterpolatedTileBaseHeightAtLocation`.
    pub fn query_interpolated_tile_base_height_at_location(&self, xy: [f64; 2]) -> (bool, f64) {
        let grid = [self.tile_region_min[0] - self.leaf_size * 0.5, self.tile_region_min[1] - self.leaf_size * 0.5];
        let normalized = [(xy[0] - grid[0]) / self.leaf_size, (xy[1] - grid[1]) / self.leaf_size];
        let corner = [normalized[0].floor() * self.leaf_size + grid[0], normalized[1].floor() * self.leaf_size + grid[1]];
        let samples = [[0.0, 0.0], [self.leaf_size, 0.0], [0.0, self.leaf_size], [self.leaf_size, self.leaf_size]];
        let mut heights = [0.0; 4];
        let mut valid = 0;
        for (i, offset) in samples.iter().enumerate() {
            let (ok, h) = self.query_tile_base_height_at_location([corner[0] + offset[0], corner[1] + offset[1]]);
            heights[i] = h;
            if ok {
                valid += 1;
            }
        }
        let fx = normalized[0] - normalized[0].floor();
        let fy = normalized[1] - normalized[1].floor();
        let bottom = heights[0] + (heights[1] - heights[0]) * fx;
        let top = heights[2] + (heights[3] - heights[2]) * fx;
        (valid == 4, bottom + (top - bottom) * fy)
    }

    /// `FViewWaterQuadTree::GetWaterLODParams`.
    pub fn water_lod_params(&self, position: [f64; 3], lod_scale: f64) -> WaterLodParams {
        let (_, water_height_for_lod) = self.query_interpolated_tile_base_height_at_location([position[0], position[1]]);
        let mut dist_to_water = (position[2] - water_height_for_lod).abs() / lod_scale;
        dist_to_water = (dist_to_water - 2.0).max(0.0);
        dist_to_water *= 2.0;
        let float_lod = (dist_to_water.log2() as f32).clamp(0.0, (self.tree_depth - 1) as f32);
        WaterLodParams {
            height_lod_factor: float_lod - float_lod.floor(),
            lowest_lod: (float_lod.floor() as i32).clamp(0, self.tree_depth - 1),
            water_height_for_lod: water_height_for_lod as f32,
        }
    }
}

/// A simulation tile's XY footprint inset by `epsilon` so the inclusive
/// `IntersectXY` selects exactly that leaf (see the module header).
pub fn tile_bounds(min_xy: [f64; 2], size: f64, z: [f64; 2], epsilon: f64) -> Box3 {
    Box3::new([min_xy[0] + epsilon, min_xy[1] + epsilon, z[0]], [min_xy[0] + size - epsilon, min_xy[1] + size - epsilon, z[1]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(tiles: &[[i32; 2]], extent: [i32; 2]) -> WaterQuadTree {
        let leaf = 100.0;
        let mut t = WaterQuadTree::init_tree([0.0, 0.0], [f64::from(extent[0] * 2) * leaf, f64::from(extent[1] * 2) * leaf], leaf, extent);
        for tile in tiles {
            t.add_water_tiles_inside_bounds(tile_bounds([f64::from(tile[0]) * leaf, f64::from(tile[1]) * leaf], leaf, [0.0, 10.0], 0.01));
        }
        t.unlock(true);
        t
    }

    fn traverse(t: &WaterQuadTree, observer: [f64; 3]) -> TraversalOutput {
        let always = |_c: [f64; 3], _e: [f64; 3]| true;
        let lod = t.water_lod_params(observer, t.leaf_size());
        let desc = TraversalDesc {
            lowest_lod: lod.lowest_lod,
            lod_count: t.tree_depth(),
            density_count: t.tree_depth().min(5),
            height_morph: lod.height_lod_factor,
            force_collapse_density_level: i32::MAX,
            lod_scale: t.leaf_size(),
            observer_position: observer,
            frustum: &always,
            lod_morphing_enabled: true,
        };
        let mut out = TraversalOutput::default();
        t.build_water_tile_instance_data(&desc, &mut out);
        out
    }

    /// Instances cover exactly the inserted leaves: total area equals the
    /// leaves' area, no instance overlaps another, and every covered leaf
    /// centre lies in exactly one instance.
    fn assert_exact_cover(t: &WaterQuadTree, tiles: &[[i32; 2]], out: &TraversalOutput) {
        let leaf = t.leaf_size();
        let area: f64 = out.staging_instance_data.iter().map(|i| i.scale[0] * i.scale[1]).sum();
        assert!((area - tiles.len() as f64 * leaf * leaf).abs() < 1e-6, "area {area} vs {} leaves", tiles.len());
        for tile in tiles {
            let c = [(f64::from(tile[0]) + 0.5) * leaf, (f64::from(tile[1]) + 0.5) * leaf];
            let hits = out
                .staging_instance_data
                .iter()
                .filter(|i| (c[0] - i.translation[0]).abs() < i.scale[0] * 0.5 && (c[1] - i.translation[1]).abs() < i.scale[1] * 0.5)
                .count();
            assert_eq!(hits, 1, "leaf {tile:?} covered {hits} times");
        }
    }

    #[test]
    fn single_leaf_creates_no_edge_neighbours() {
        let t = tree(&[[3, 3]], [4, 4]);
        let out = traverse(&t, [350.0, 350.0, 20.0]);
        assert_eq!(out.instance_count, 1);
        assert_exact_cover(&t, &[[3, 3]], &out);
    }

    #[test]
    fn river_strip_is_covered_exactly_at_any_observer() {
        // A diagonal-ish meandering strip two tiles wide.
        let mut tiles = Vec::new();
        for x in 0..60 {
            let y = 20 + ((x as f64 * 0.2).sin() * 8.0) as i32;
            tiles.push([x, y]);
            tiles.push([x, y + 1]);
        }
        tiles.sort();
        tiles.dedup();
        let t = tree(&tiles, [32, 32]);
        for observer in [[0.0, 2000.0, 50.0], [3000.0, 2100.0, 5000.0], [-4000.0, -4000.0, 20.0]] {
            let out = traverse(&t, observer);
            assert_exact_cover(&t, &tiles, &out);
        }
    }

    #[test]
    fn complete_block_collapses_far_away_and_refines_near() {
        let tiles: Vec<[i32; 2]> = (0..16).flat_map(|x| (0..16).map(move |y| [x, y])).collect();
        let t = tree(&tiles, [8, 8]);
        let far = traverse(&t, [100_000.0, 100_000.0, 10.0]);
        let near = traverse(&t, [50.0, 50.0, 10.0]);
        assert_exact_cover(&t, &tiles, &far);
        assert_exact_cover(&t, &tiles, &near);
        assert!(far.instance_count < near.instance_count, "far {} near {}", far.instance_count, near.instance_count);
        // LOD rises with distance from the observer.
        let near_lod = near.staging_instance_data.iter().min_by(|a, b| {
            let d = |i: &StagingInstance| (i.translation[0] - 50.0).hypot(i.translation[1] - 50.0);
            d(a).total_cmp(&d(b))
        });
        assert_eq!(near_lod.map(|i| i.lod_level), Some(0));
    }

    #[test]
    fn lod_distance_and_height_params_match_upstream_formulas() {
        assert_eq!(WaterQuadTree::lod_distance(0, 100.0), 200.0);
        assert_eq!(WaterQuadTree::lod_distance(3, 100.0), 1600.0);
        let t = tree(&[[0, 0], [1, 0], [0, 1], [1, 1]], [2, 2]);
        // Within 2 LODScale of the water: lowest LOD 0, no height morph.
        let low = t.water_lod_params([100.0, 100.0, 110.0], 100.0);
        assert_eq!((low.lowest_lod, low.height_lod_factor), (0, 0.0));
        // (|dz|/scale - 2) * 2 = 6 -> log2 6 = 2.585 -> lowest 2, morph 0.585.
        let high = t.water_lod_params([100.0, 100.0, 10.0 + 500.0], 100.0);
        assert_eq!(high.lowest_lod, 2.min(t.tree_depth() - 1));
    }
}

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
pub mod appearance;
pub mod interaction;
pub mod surfaces;
pub mod bathymetry;
mod blue_water;
mod blue_wake;
pub use blue_wake::{ShipClass, StrikeKind, WakeCommand};
pub mod eye_wave;
mod native_waves;
mod native_optics;
pub use native_optics::BlueWaterOptics;
pub use native_waves::BlueWaterWaves;
mod original_surface;
pub mod coastal;
pub mod upstream_coast;
pub mod gerstner_contact;
mod gerstner_patterns;
mod lakes;
mod lake_wakes;
pub use lakes::LakeSettings;
pub mod sky_extend;
pub mod volume;
mod footprint_index;
#[cfg(test)]
#[path = "ocean/footprint_perf_tests.rs"]
mod footprint_perf_tests;
pub use authoring::{links, AtmosphericWind, OceanAuthoring, OceanWind};
pub const OCEAN_CASCADES: usize = 3;
pub const OCEAN_DEFAULT_MAP_SIZE: u32 = 1024;
pub const OCEAN_MAX_SURFACES: usize = 8;
/// Shared world-space sparse-footprint tile size. At the coastal bake's
/// default 16-unit spacing this is a 16x16-cell allocation tile.
pub const WATER_FOOTPRINT_TILE_SIZE: f32 = 256.0;
// Five existing GPU passes per cascade.  No shader/FFT algorithm changes.
const OCEAN_PASSES_PER_CASCADE: usize = 5;
const OCEAN_FFT_PASSES: usize = OCEAN_CASCADES * OCEAN_PASSES_PER_CASCADE;
// At low render FPS a strict one-pass-per-frame limit would turn a 24 Hz ocean
// into a ~4 Hz ocean at 60 FPS. Catch up modestly, never flush all 15 passes
// simply because the next simulation update is due.
const OCEAN_MAX_PASSES_PER_FRAME: usize = 5;
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

/// Legacy half-width of every clipmap level, in cells. Authored oceans keep
/// this exact Godot-style footprint. Automatically promoted water may use a
/// smaller camera-centred ring width when its displacement amplitude is low.
const CLIPMAP_HALF_CELLS: i32 = 128;
/// The original six-level design remains the authored-ocean profile and, more
/// importantly, the CPU clipmap-centre snap cadence. Calm promoted water may
/// add two cheaper outer rings so shrinking the inner LOD radii never shrinks
/// the world-space coverage of the mesh.
const CLIPMAP_LEVELS: u32 = 6;
const PROMOTED_CLIPMAP_MAX_LEVELS: u32 = CLIPMAP_LEVELS + 2;
const PROMOTED_CLIPMAP_MIN_HALF_CELLS: i32 = 32;
const PROMOTED_CLIPMAP_CELL_QUANTUM: i32 = 8;
const PROMOTED_CLIPMAP_FULL_DETAIL_AMPLITUDE: f32 = 64.0;

/// Capture-only CPU timings for native ocean mesh preparation.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeMeshCpuStats {
    /// Entire native surface preparation, inclusive of mesh build/upload.
    pub prepare_ms: f64,
    /// CPU geometry generation and admission queries; excludes buffer uploads.
    pub build_ms: f64,
    /// CPU buffer growth and write_buffer calls, not GPU transfer completion.
    pub upload_ms: f64,
    pub rebuilds: u32,
    pub vertices: u64,
    pub indices: u64,
    pub upload_bytes: u64,
}

/// Per-frame ocean submission counters.
///
/// These exist because the ocean's cost does not scale with how much water is
/// on screen: the clipmap is camera-centred, so one visible sliver of a
/// promoted surface submits that surface's whole mesh, and the shape mask then
/// discards the fragments outside the real outline. `triangles` is the number
/// that makes that visible; `clipmap_batches` vs `clipmaps_drawn` shows how
/// much of the PVS set the per-surface claim collapsed.
#[derive(Clone, Copy, Debug, Default)]
pub struct OceanDrawStats {
    pub native_cpu: NativeMeshCpuStats,
    /// CPU footprint queries/index regeneration, including work before acquire.
    pub topology_ms: f64,
    /// Promoted surfaces the loaded world carries, visible or not.
    pub promoted_surfaces: u32,
    /// Authored (`r_oceanAuthoring`) oceans, each of which owns a *separate*
    /// FFT simulator rather than sharing the promoted one.
    pub authored_oceans: u32,
    /// Batches carrying a clipmap that survived area + frustum culling.
    pub clipmap_batches: u32,
    /// Clipmap draws actually encoded (one per surface id per pass).
    pub clipmaps_drawn: u32,
    /// Draws using Blue Water's original surface shader and upstream geometry.
    pub original_surfaces: u32,
    /// Mesh-free fullscreen triangles drawn for qualifying ocean pools.
    pub analytic_draws: u32,
    pub rejected_area: u32,
    pub rejected_frustum: u32,
    /// Collapsed by `claim_ocean_clipmap` because the surface was already drawn.
    pub rejected_duplicate: u32,
    /// Triangles submitted by those clipmap draws.
    pub triangles: u64,
    /// Debug: full-detail triangles for the same native PVS mask.
    pub adaptive_mesh_baseline_triangles: u64,
    /// Debug: actual native indexed triangles submitted.
    pub adaptive_mesh_selected_triangles: u64,
    /// Debug: native draws included in the comparison.
    pub adaptive_mesh_counted_slots: u32,
    /// Debug: native slots without a measured full-detail baseline.
    pub adaptive_mesh_uncounted_slots: u32,
    /// Finest live clipmap level and ring width, over the drawn surfaces.
    pub minimum_level: u32,
    pub half_cells: i32,
    /// Underwater optics capture/composite ran this frame.
    pub optics_active: bool,
    pub submerged: bool,
    /// Pixels the optics capture actually shaded. Compare against the viewport:
    /// this is the number the scissor is meant to shrink, and 0 means the whole
    /// pass was skipped because nothing could observe it.
    pub optics_pixels: u64,
    /// The FFT/wind-foam encode was skipped: no water seen recently.
    pub sim_hidden: bool,
    /// The capture scissor the frame computed, as x/y/w/h, before clamping.
    pub optics_rect: [i32; 4],
    /// A visible surface's bounds crossed the eye, so no screen rectangle
    /// bounds it and the capture had to stay full-screen.
    pub optics_unbounded: bool,
    /// Ocean samples that passed depth, from the GPU occlusion query. This is
    /// the signal the gates use; compare it against `clipmaps_drawn`, which is
    /// a bounds test and reads 1 even when this is 0.
    pub visible_samples: u64,
    /// No occlusion readback has landed yet, so visibility is assumed.
    pub visibility_primed: bool,
}

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

/// Runtime-rebuildable index topology for automatically promoted water.
///
/// The vertex grids are uploaded once at their maximum useful widths. The
/// index list can then be rewritten cheaply when local amplitude or the
/// camera's true 3D distance from the water selects a different LOD profile.
/// This keeps the expensive vertex allocation out of slider/camera updates
/// while still reducing the triangles the GPU actually submits.
#[derive(Clone, Debug)]
pub struct PromotedClipmapLayout {
    pub base_spacing: f32,
    pub extent: f32,
    pub level_vertex_bases: [u32; 8],
    pub level_vertex_half_cells: [i32; 8],
}

#[derive(Clone, Debug)]
pub struct RuntimePromotedClipmap {
    pub vertices: Vec<crate::scene::GpuVertex>,
    pub indices: Vec<u32>,
    pub index_capacity: usize,
    pub coarsest_spacing: f32,
    pub layout: PromotedClipmapLayout,
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
fn clipmap_levels_for(base_spacing: f32, extent: f32, half_cells: i32, max_levels: u32) -> u32 {
    if !extent.is_finite() {
        return max_levels;
    }
    let reach = half_cells as f32 - 0.5;
    (1..=max_levels)
        .find(|&levels| base_spacing * (1u32 << (levels - 1)) as f32 * reach >= extent)
        .unwrap_or(max_levels)
}

/// Camera-distance LOD radius for automatically promoted water.
///
/// Amplitude is the useful signal here: a five-unit ripple does not need the
/// same 1280-unit radius of 10-unit triangles as a 64+-unit ocean swell. Keep
/// the exact finest spacing beside the viewer, but make each 2:1 clipmap ring
/// physically narrower for calm water. `sqrt` is deliberately conservative:
/// medium seas recover detail radius faster than a linear rule would.
pub fn promoted_clipmap_half_cells(amplitude: f32) -> i32 {
    if !amplitude.is_finite() {
        return CLIPMAP_HALF_CELLS;
    }
    let minimum_amplitude = appearance::MIN_PROMOTED_AMPLITUDE;
    let t = ((amplitude.max(minimum_amplitude) - minimum_amplitude)
        / (PROMOTED_CLIPMAP_FULL_DETAIL_AMPLITUDE - minimum_amplitude))
        .clamp(0.0, 1.0)
        .sqrt();
    let cells = PROMOTED_CLIPMAP_MIN_HALF_CELLS as f32
        + (CLIPMAP_HALF_CELLS - PROMOTED_CLIPMAP_MIN_HALF_CELLS) as f32 * t;
    (((cells / PROMOTED_CLIPMAP_CELL_QUANTUM as f32).ceil() as i32)
        * PROMOTED_CLIPMAP_CELL_QUANTUM)
        .clamp(PROMOTED_CLIPMAP_MIN_HALF_CELLS, CLIPMAP_HALF_CELLS)
}

fn promoted_level_half_cells(level: u32, half_cells: i32) -> i32 {
    if level < CLIPMAP_LEVELS {
        half_cells
    } else {
        // Extra rings exist only to preserve distant coverage after narrowing
        // the near-field profile. At twice the spacing, half the previous
        // width reaches the same world-space radius; keep at least the calm
        // profile width so low-amplitude water continues expanding outward.
        PROMOTED_CLIPMAP_MIN_HALF_CELLS.max(half_cells / 2)
    }
}

fn promoted_levels_for(base_spacing: f32, extent: f32, half_cells: i32) -> u32 {
    for level in 0..PROMOTED_CLIPMAP_MAX_LEVELS {
        let spacing = base_spacing * (1u32 << level) as f32;
        let half = promoted_level_half_cells(level, half_cells) as f32;
        if spacing * (half - 0.5) >= extent {
            return level + 1;
        }
    }
    PROMOTED_CLIPMAP_MAX_LEVELS
}

/// The coarsest active cell size for a promoted clipmap at this amplitude.
/// Kept public because the per-pool render uniform must use the same lattice
/// as the CPU index topology when the live amplitude slider changes it.
pub fn promoted_clipmap_outer_spacing(base_spacing: f32, extent: f32, amplitude: f32) -> f32 {
    let half_cells = promoted_clipmap_half_cells(amplitude);
    let levels = promoted_levels_for(base_spacing, extent, half_cells);
    base_spacing * (1u32 << (levels.saturating_sub(1))) as f32
}

impl PromotedClipmapLayout {
    fn levels_for_half_cells(&self, half_cells: i32) -> u32 {
        promoted_levels_for(self.base_spacing, self.extent, half_cells)
    }

    pub fn outer_spacing_for_half_cells(&self, half_cells: i32) -> f32 {
        let levels = self.levels_for_half_cells(half_cells);
        self.base_spacing * (1u32 << levels.saturating_sub(1)) as f32
    }

    /// Conservative XY half-width of this finite camera-following grid.
    pub fn coverage_radius(&self, half_cells: i32) -> f32 {
        let level = self.levels_for_half_cells(half_cells).saturating_sub(1);
        self.base_spacing * (1u32 << level) as f32
            * promoted_level_half_cells(level, half_cells) as f32
    }

    /// Exact outer radius of the levels that survived underwater distance culling.
    /// The analytic continuation starts only outside this square; it never
    /// overlays the displaced geometric clipmap or depends on arbitrary range.
    pub fn coverage_radius_for_limit(&self, half_cells: i32, minimum_level: u32, level_limit: u32) -> f32 {
        let level = self.levels_for_half_cells(half_cells)
            .min(level_limit.max(minimum_level.saturating_add(1))).saturating_sub(1);
        self.base_spacing * (1u32 << level) as f32
            * promoted_level_half_cells(level, half_cells) as f32
    }

    /// Select the finest level worth drawing from the shortest 3D distance to
    /// the water footprint. This is the piece the old camera-centred clipmap
    /// missed: altitude (JKA Z / renderer Y) must count just like horizontal
    /// separation, otherwise flying directly above a sea pins maximum LOD.
    pub fn minimum_level_for_distance(&self, half_cells: i32, distance: f32) -> u32 {
        let levels = self.levels_for_half_cells(half_cells);
        if levels <= 1 || !distance.is_finite() {
            return 0;
        }
        let mut level = 0;
        while level + 1 < levels {
            let spacing = self.base_spacing * (1u32 << level) as f32;
            let half = promoted_level_half_cells(level, half_cells) as f32;
            let reach = spacing * (half - 0.5);
            if distance < reach {
                break;
            }
            level += 1;
        }
        level
    }

    /// How many levels (counted from level 0) can contain anything within
    /// `radius` horizontally of a viewer who is `offset` from the clipmap centre.
    /// Level `n` starts where level `n - 1` ends.
    pub fn levels_within(&self, half_cells: i32, offset: f32, radius: f32) -> u32 {
        let levels = self.levels_for_half_cells(half_cells);
        for level in 1..levels {
            let previous = level - 1;
            let reach = self.base_spacing * (1u32 << previous) as f32
                * promoted_level_half_cells(previous, half_cells) as f32;
            if reach - offset >= radius {
                return level;
            }
        }
        levels
    }

    fn index_count_for(&self, half_cells: i32, minimum_level: u32) -> usize {
        let levels = self.levels_for_half_cells(half_cells);
        let first = minimum_level.min(levels.saturating_sub(1));
        let mut previous_half = 0i32;
        let mut cells = 0usize;
        for level in first..levels {
            let half = promoted_level_half_cells(level, half_cells);
            let hole = if level == first { 0 } else { previous_half / 2 };
            let side_cells = usize::try_from(half * 2).unwrap_or(0);
            let hole_cells = usize::try_from(hole * 2).unwrap_or(0);
            cells = cells.saturating_add(
                side_cells
                    .saturating_mul(side_cells)
                    .saturating_sub(hole_cells.saturating_mul(hole_cells)),
            );
            previous_half = half;
        }
        cells.saturating_mul(6)
    }

    /// Build only the indices used by the selected live profile. The uploaded
    /// vertex grids are shared by all profiles; a lower `minimum_level` fills
    /// the centre at that level and omits every finer grid entirely.
    pub fn indices_for(&self, half_cells: i32, minimum_level: u32) -> Vec<u32> {
        let levels = self.levels_for_half_cells(half_cells);
        let first = minimum_level.min(levels.saturating_sub(1));
        let mut indices = Vec::with_capacity(self.index_count_for(half_cells, minimum_level));
        let mut previous_half = 0i32;
        for level in first..levels {
            let spacing_half = promoted_level_half_cells(level, half_cells);
            let vertex_half = self.level_vertex_half_cells[level as usize];
            debug_assert!(spacing_half <= vertex_half);
            let hole = if level == first { 0 } else { previous_half / 2 };
            let side = (vertex_half * 2 + 1) as u32;
            let base = self.level_vertex_bases[level as usize];
            let index = |c: i32, r: i32| {
                base + (r + vertex_half) as u32 * side + (c + vertex_half) as u32
            };
            for row in -spacing_half..spacing_half {
                for column in -spacing_half..spacing_half {
                    if hole > 0
                        && column >= -hole
                        && column < hole
                        && row >= -hole
                        && row < hole
                    {
                        continue;
                    }
                    let a = index(column, row);
                    let b = index(column + 1, row);
                    let c = index(column, row + 1);
                    let d = index(column + 1, row + 1);
                    indices.extend_from_slice(&[a, c, b, b, c, d]);
                }
            }
            previous_half = spacing_half;
        }
        indices
    }

    /// Builds the same live LOD topology while omitting cells proven dry by
    /// the promoted surface's exact footprint. This runs only when the snapped
    /// clipmap centre or an existing LOD selector changes.
    pub fn indices_for_footprint(
        &self,
        half_cells: i32,
        minimum_level: u32,
        centre: [f32; 2],
        surface: OceanSurface,
        masks: &OceanMasks,
        mask_slot: Option<usize>,
        pvs_tiles: Option<&OceanPvsTileMask>,
    ) -> Vec<u32> {
        self.indices_for_footprint_with_sky(
            half_cells, minimum_level, u32::MAX, centre, surface, masks, mask_slot, pvs_tiles, None)
    }

    /// The sky mode reuses the same uploaded grids and indexed draw, but its
    /// snapped centre follows the camera outside the BSP and isn't clamped to
    /// the original water. No per-frame mesh or GPU buffer allocation occurs.
    pub fn indices_for_footprint_with_sky(
        &self,
        half_cells: i32,
        minimum_level: u32,
        // Draw no level past this one (counted from 0): outer rings that water
        // absorption already hides. The lattice (`outer_spacing`) is unchanged.
        level_limit: u32,
        centre: [f32; 2],
        surface: OceanSurface,
        masks: &OceanMasks,
        mask_slot: Option<usize>,
        pvs_tiles: Option<&OceanPvsTileMask>,
        sky: Option<&sky_extend::SkyExtension>,
    ) -> Vec<u32> {
        let levels = self.levels_for_half_cells(half_cells);
        let first = minimum_level.min(levels.saturating_sub(1));
        let outer_spacing = self.base_spacing * (1u32 << (levels.saturating_sub(1))) as f32;
        let levels = levels.min(level_limit.max(first + 1));
        let mut indices = Vec::with_capacity(self.index_count_for(half_cells, minimum_level));
        let mut previous_half = 0i32;
        for level in first..levels {
            let spacing_half = promoted_level_half_cells(level, half_cells);
            let vertex_half = self.level_vertex_half_cells[level as usize];
            debug_assert!(spacing_half <= vertex_half);
            let spacing = self.base_spacing * (1u32 << level) as f32;
            let hole = if level == first { 0 } else { previous_half / 2 };
            let side = (vertex_half * 2 + 1) as u32;
            let base = self.level_vertex_bases[level as usize];
            let index = |c: i32, r: i32| {
                base + (r + vertex_half) as u32 * side + (c + vertex_half) as u32
            };
            let position = |c: i32, r: i32| {
                let mut offset = [c as f32 * spacing, r as f32 * spacing];
                let on_outer = c.abs().max(r.abs()) >= spacing_half;
                if on_outer && spacing < outer_spacing - 0.25 {
                    let parent = spacing * 2.0;
                    offset[0] = (offset[0] / parent).round() * parent;
                    offset[1] = (offset[1] / parent).round() * parent;
                }
                let p = [centre[0] + offset[0], centre[1] + offset[1]];
                if sky.is_some() { p } else {
                    [p[0].clamp(surface.minimum[0], surface.maximum[0]),
                     p[1].clamp(surface.minimum[1], surface.maximum[1])]
                }
            };
            for row in -spacing_half..spacing_half {
                for column in -spacing_half..spacing_half {
                    if hole > 0
                        && column >= -hole
                        && column < hole
                        && row >= -hole
                        && row < hole
                    {
                        continue;
                    }
                    let corners = [
                        position(column, row),
                        position(column + 1, row),
                        position(column, row + 1),
                        position(column + 1, row + 1),
                    ];
                    let lo = [
                        corners.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min),
                        corners.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min),
                    ];
                    let hi = [
                        corners.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max),
                        corners.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max),
                    ];
                    if hi[0] - lo[0] <= 1.0e-4 || hi[1] - lo[1] <= 1.0e-4 {
                        continue;
                    }
                    if !promoted_ocean_cell_may_contain(
                        surface, masks, mask_slot, pvs_tiles, sky, lo, hi,
                    ) {
                        continue;
                    }
                    let a = index(column, row);
                    let b = index(column + 1, row);
                    let c = index(column, row + 1);
                    let d = index(column + 1, row + 1);
                    indices.extend_from_slice(&[a, c, b, b, c, d]);
                }
            }
            previous_half = spacing_half;
        }
        indices
    }

    pub fn maximum_index_count(&self) -> usize {
        let mut maximum = 0usize;
        let mut half = PROMOTED_CLIPMAP_MIN_HALF_CELLS;
        while half <= CLIPMAP_HALF_CELLS {
            let levels = self.levels_for_half_cells(half);
            for first in 0..levels {
                maximum = maximum.max(self.index_count_for(half, first));
            }
            half += PROMOTED_CLIPMAP_CELL_QUANTUM;
        }
        maximum
    }
}

/// Shortest render-space distance from the camera to the undisplaced
/// rectangular water footprint. Renderer Y is JKA Z, so flying above the sea
/// naturally increases this distance even when X/Z remain inside its bounds.
#[cfg(test)]
pub fn promoted_surface_distance(camera: [f32; 3], surface: OceanSurface) -> f32 {
    let axis_distance = |value: f32, minimum: f32, maximum: f32| {
        if value < minimum {
            minimum - value
        } else if value > maximum {
            value - maximum
        } else {
            0.0
        }
    };
    let dx = axis_distance(camera[0], surface.minimum[0], surface.maximum[0]);
    let dy = (camera[1] - surface.plane_height).abs();
    let dz = axis_distance(camera[2], surface.minimum[1], surface.maximum[1]);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// True distance to the source water footprint when it is available. The
/// AABB calculation remains the conservative fallback for authored/legacy data.
pub fn promoted_footprint_distance(
    camera: [f32; 3],
    surface: OceanSurface,
    masks: &OceanMasks,
    slot: usize,
) -> f32 {
    let p = [camera[0], camera[2]];
    let nearest = masks.closest_point(slot, p).unwrap_or([
        p[0].clamp(surface.minimum[0], surface.maximum[0]),
        p[1].clamp(surface.minimum[1], surface.maximum[1]),
    ]);
    let horizontal_sq = (p[0] - nearest[0]).powi(2) + (p[1] - nearest[1]).powi(2);
    (horizontal_sq + (camera[1] - surface.plane_height).powi(2)).sqrt()
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
    build_clipmap_profile(
        base_spacing,
        plane_height,
        footprint_minimum,
        footprint_maximum,
        mask_slot,
        CLIPMAP_HALF_CELLS,
        CLIPMAP_LEVELS,
    )
}

/// Amplitude-aware clipmap used only for ordinary BSP water promoted into the
/// FFT ocean. Authored oceans deliberately keep `build_clipmap`'s reference
/// profile. A calm pool retains the same 0.25 m / 1 m innermost cell spacing,
/// but its fine rings end much sooner; extra very-coarse rings preserve the old
/// maximum world-space reach without paying for huge near-field grids.
#[cfg(test)]
pub fn build_promoted_clipmap(
    base_spacing: f32,
    plane_height: f32,
    footprint_minimum: [f32; 2],
    footprint_maximum: [f32; 2],
    mask_slot: Option<usize>,
    amplitude: f32,
) -> OceanClipmap {
    build_clipmap_profile(
        base_spacing,
        plane_height,
        footprint_minimum,
        footprint_maximum,
        mask_slot,
        promoted_clipmap_half_cells(amplitude),
        PROMOTED_CLIPMAP_MAX_LEVELS,
    )
}

/// Per-level vertex offsets and half-widths of the promoted grid. They depend
/// only on the constant level profile, never on the pool.
fn promoted_clipmap_vertex_levels() -> ([u32; PROMOTED_CLIPMAP_MAX_LEVELS as usize], [i32; PROMOTED_CLIPMAP_MAX_LEVELS as usize]) {
    let mut bases = [0u32; PROMOTED_CLIPMAP_MAX_LEVELS as usize];
    let mut halves = [0i32; PROMOTED_CLIPMAP_MAX_LEVELS as usize];
    let mut count = 0u32;
    for level in 0..PROMOTED_CLIPMAP_MAX_LEVELS {
        let vertex_half = if level < CLIPMAP_LEVELS { CLIPMAP_HALF_CELLS } else { CLIPMAP_HALF_CELLS / 2 };
        bases[level as usize] = count;
        halves[level as usize] = vertex_half;
        count += ((vertex_half * 2 + 1) as u32).pow(2);
    }
    (bases, halves)
}

/// The promoted-water vertex grid for one mesh quality. It is pool-neutral:
/// plane height, footprint and mask slot come from the pool's promoted render
/// uniform (`spray_planes[0]`, `authored_bounds`), so every promoted pool of a
/// map indexes this one grid instead of uploading ~25 MiB of its own.
pub fn promoted_clipmap_grid(base_spacing: f32) -> Vec<crate::scene::GpuVertex> {
    let (_, halves) = promoted_clipmap_vertex_levels();
    let mut vertices = Vec::new();
    for level in 0..PROMOTED_CLIPMAP_MAX_LEVELS {
        let vertex_half = halves[level as usize];
        let spacing = base_spacing * (1u32 << level) as f32;
        let side = (vertex_half * 2 + 1) as usize;
        for row in 0..side {
            for column in 0..side {
                let cell_x = column as i32 - vertex_half;
                let cell_z = row as i32 - vertex_half;
                vertices.push(crate::scene::GpuVertex {
                    position: [cell_x as f32 * spacing, 0.0, cell_z as f32 * spacing],
                    uv: [0.0; 2],
                    lightmap_uv: [0.0; 2],
                    normal: [0.0, 1.0, 0.0],
                    // Dynamic promoted vertices keep their grid capacity in G.
                    // Extra coverage-only levels use a negative sign so the
                    // shader applies max(32, live_half/2) to those levels.
                    color: [
                        spacing,
                        if level < CLIPMAP_LEVELS { vertex_half as f32 } else { -(vertex_half as f32) },
                        0.0,
                        -1.0,
                    ],
                    alpha_cutoff: OCEAN_CLIPMAP_VERTEX_MARKER,
                });
            }
        }
    }
    vertices
}

/// Upload-once promoted-water clipmap topology for one pool. Its indices are
/// relative to `promoted_clipmap_grid`; `vertices` is left empty because the
/// grid is shared. Runtime amplitude and camera-distance changes rewrite those
/// indices in-place without rebuilding the rest of the BSP world.
pub fn build_runtime_promoted_clipmap_topology(
    base_spacing: f32,
    footprint_minimum: [f32; 2],
    footprint_maximum: [f32; 2],
    amplitude: f32,
) -> RuntimePromotedClipmap {
    let extent = (footprint_maximum[0] - footprint_minimum[0])
        .max(footprint_maximum[1] - footprint_minimum[1]);
    let (level_vertex_bases, level_vertex_half_cells) = promoted_clipmap_vertex_levels();
    let layout = PromotedClipmapLayout {
        base_spacing,
        extent,
        level_vertex_bases,
        level_vertex_half_cells,
    };
    let half_cells = promoted_clipmap_half_cells(amplitude);
    let indices = layout.indices_for(half_cells, 0);
    let index_capacity = layout.maximum_index_count();
    RuntimePromotedClipmap {
        vertices: Vec::new(),
        indices,
        index_capacity,
        coarsest_spacing: base_spacing * (1u32 << (CLIPMAP_LEVELS - 1)) as f32,
        layout,
    }
}

/// Topology plus its (pool-neutral) grid, for tests that inspect both.
#[cfg(test)]
pub fn build_runtime_promoted_clipmap(
    base_spacing: f32,
    _plane_height: f32,
    footprint_minimum: [f32; 2],
    footprint_maximum: [f32; 2],
    _mask_slot: Option<usize>,
    amplitude: f32,
) -> RuntimePromotedClipmap {
    let mut clipmap = build_runtime_promoted_clipmap_topology(
        base_spacing, footprint_minimum, footprint_maximum, amplitude,
    );
    clipmap.vertices = promoted_clipmap_grid(base_spacing);
    clipmap
}

fn build_clipmap_profile(
    base_spacing: f32,
    plane_height: f32,
    footprint_minimum: [f32; 2],
    footprint_maximum: [f32; 2],
    mask_slot: Option<usize>,
    half_cells: i32,
    max_levels: u32,
) -> OceanClipmap {
    debug_assert!(half_cells > 0 && half_cells % 2 == 0);
    let mask_slot = mask_slot.map_or(-1.0, |slot| slot as f32);
    let extent = (footprint_maximum[0] - footprint_minimum[0])
        .max(footprint_maximum[1] - footprint_minimum[1]);
    let levels = clipmap_levels_for(base_spacing, extent, half_cells, max_levels);
    let snap_step = base_spacing * (1u32 << (levels - 1)) as f32;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for level in 0..levels {
        let spacing = base_spacing * (1u32 << level) as f32;
        // Every outer level's hole is half its own cell radius. At twice the
        // spacing that lands exactly on the previous level's outer boundary,
        // even when calm-water LOD uses a narrower ring than the reference.
        let hole = if level == 0 { 0 } else { half_cells / 2 };
        // Only a lone level has no neighbour whose hole it must match, so only
        // it may shrink below the selected profile width.
        let half = if levels == 1 {
            (((extent + spacing * 0.5) / spacing).ceil() as i32).clamp(1, half_cells)
        } else {
            half_cells
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
        // Keep the CPU centre's update/snap cadence at the original design.
        // Adaptive promoted meshes may have extra coarse outer rings, but using
        // those for camera snapping would make the fine inner grid jump by
        // thousands of units. The shader still re-snaps each surface to its
        // actual outermost lattice through color.b.
        coarsest_spacing: base_spacing * (1u32 << (CLIPMAP_LEVELS - 1)) as f32,
    }
}

/// Capacity of the shape-mask triangle store, in vec4s. A triangle takes two.
pub const OCEAN_MASK_VEC4S: usize = 512;
/// Coarse exact-acceleration grid for promoted-water outlines. Each tile uses
/// two bits: 0 means test the source triangles, 1 is definitely outside and 2
/// is definitely inside. Boundary tiles therefore retain the original answer.
pub const OCEAN_MASK_TILE_AXIS: usize = 8;
/// Finer CPU-only grid used when pruning dry cells from the moving clipmap.
/// It never reaches a shader uniform, so improving topology precision costs
/// only 256 bytes per masked surface.
pub const OCEAN_PRUNE_TILE_AXIS: usize = 32;
pub const OCEAN_PRUNE_TILE_WORDS: usize = OCEAN_PRUNE_TILE_AXIS * OCEAN_PRUNE_TILE_AXIS * 2 / 32;
/// One bit per fine tile. PVS footprints need only visible/not-visible rather
/// than the outline accelerator's three states, so one u32 represents a row.
pub type OceanPvsTileMask = [u32; OCEAN_PRUNE_TILE_AXIS];

#[derive(Clone, Debug)]
struct OceanPvsPiece {
    slot: usize,
    tiles: OceanPvsTileMask,
}

/// Exact-PVS water-piece footprints, indexed in parallel with `full_batches`.
/// The renderer unions only the entries in the current cluster's existing FULL
/// visibility list, so memory scales with water pieces rather than with every
/// ocean x every BSP cluster.
#[derive(Clone, Debug, Default)]
pub struct OceanPvsFootprints {
    pieces: Vec<Option<OceanPvsPiece>>,
    known: [bool; OCEAN_MAX_SURFACES],
}

impl OceanPvsFootprints {
    pub fn with_batch_count(batch_count: usize) -> Self {
        Self {
            pieces: vec![None; batch_count],
            known: [false; OCEAN_MAX_SURFACES],
        }
    }

    /// Records the exact water geometry carried by one FULL/PVS batch.
    pub fn add_piece(
        &mut self,
        batch: usize,
        slot: usize,
        surface: OceanSurface,
        triangles: impl IntoIterator<Item = MaskTriangle>,
    ) {
        if batch >= self.pieces.len() || slot >= OCEAN_MAX_SURFACES {
            return;
        }
        let size = [
            surface.maximum[0] - surface.minimum[0],
            surface.maximum[1] - surface.minimum[1],
        ];
        let mut tiles = [0u32; OCEAN_PRUNE_TILE_AXIS];
        if size[0] <= 1.0e-4 || size[1] <= 1.0e-4 {
            tiles.fill(u32::MAX);
        } else {
            let tile = |value: f32, axis: usize| {
                (((value - surface.minimum[axis]) / size[axis]
                    * OCEAN_PRUNE_TILE_AXIS as f32)
                    .floor() as i32)
                    .clamp(0, OCEAN_PRUNE_TILE_AXIS as i32 - 1) as usize
            };
            for triangle in triangles {
                if triangle.iter().flatten().any(|value| !value.is_finite()) {
                    tiles.fill(u32::MAX);
                    break;
                }
                let lo = [0, 1].map(|axis| {
                    triangle
                        .iter()
                        .map(|point| point[axis])
                        .fold(f32::INFINITY, f32::min)
                        - MASK_TOLERANCE
                });
                let hi = [0, 1].map(|axis| {
                    triangle
                        .iter()
                        .map(|point| point[axis])
                        .fold(f32::NEG_INFINITY, f32::max)
                        + MASK_TOLERANCE
                });
                let (x0, x1, y0, y1) =
                    (tile(lo[0], 0), tile(hi[0], 0), tile(lo[1], 1), tile(hi[1], 1));
                for row in &mut tiles[y0..=y1] {
                    for x in x0..=x1 {
                        *row |= 1u32 << x;
                    }
                }
            }
        }
        self.known[slot] = true;
        self.pieces[batch] = Some(OceanPvsPiece { slot, tiles });
    }

    /// Unions water tiles from the renderer's exact visible-batch list. `None`
    /// means this surface lacked exact pieces and must remain conservative;
    /// `Some(all-zero)` means the surface is completely outside this PVS.
    pub fn visible_tiles(
        &self,
        visible_batches: &[usize],
    ) -> [Option<OceanPvsTileMask>; OCEAN_MAX_SURFACES] {
        let mut visible = std::array::from_fn(|slot| self.known[slot].then_some([0; OCEAN_PRUNE_TILE_AXIS]));
        for &batch in visible_batches {
            let Some(piece) = self.pieces.get(batch).and_then(Option::as_ref) else {
                continue;
            };
            let Some(surface) = visible[piece.slot].as_mut() else {
                continue;
            };
            for (destination, source) in surface.iter_mut().zip(piece.tiles) {
                *destination |= source;
            }
        }
        visible
    }
}

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
    pub bounds: [[f32; 4]; OCEAN_MAX_SURFACES],
    pub tiles: [[u32; 4]; OCEAN_MAX_SURFACES],
    pub prune_tiles: [[u32; OCEAN_PRUNE_TILE_WORDS]; OCEAN_MAX_SURFACES],
    pub triangles: Vec<[f32; 4]>,
    /// CPU-side exact sparse footprints. These do not enter the shader
    /// uniform; they guide clipmap centering, LOD distance, and index pruning.
    pub footprints: [Option<std::sync::Arc<WaterFootprint>>; OCEAN_MAX_SURFACES],
    footprint_indices: [Option<std::sync::Arc<footprint_index::FootprintIndex>>; OCEAN_MAX_SURFACES],
    /// Shoreline and hole-rim segments `[x0, z0, x1, z1]` per masked slot, for
    /// the CPU-only water culling queries. Not uploaded.
    boundaries: [Option<std::sync::Arc<Vec<[f32; 4]>>>; OCEAN_MAX_SURFACES],
}

/// CPU twin of the promoted vertex shader's two-stage centre snapping. The
/// global clipmap moves on the legacy coarse lattice; each promoted surface
/// then clamps that point into its bounds and snaps it to its live outer ring.
#[cfg(test)]
pub fn promoted_clipmap_centre(
    camera: [f32; 3],
    surface: OceanSurface,
    global_spacing: f32,
    outer_spacing: f32,
) -> [f32; 2] {
    let global = global_spacing.max(1.0);
    let outer = outer_spacing.max(1.0);
    let camera = [
        (camera[0] / global).round() * global,
        (camera[2] / global).round() * global,
    ];
    [
        (camera[0].clamp(surface.minimum[0], surface.maximum[0]) / outer).round() * outer,
        (camera[1].clamp(surface.minimum[1], surface.maximum[1]) / outer).round() * outer,
    ]
}

/// Footprint-aware equivalent of [`promoted_clipmap_centre`]. An L-shaped
/// ocean therefore keeps its finest rings on the nearest real water instead
/// of centering them in the dry corner of its enclosing rectangle.
pub fn promoted_footprint_clipmap_centre(
    camera: [f32; 3],
    surface: OceanSurface,
    global_spacing: f32,
    outer_spacing: f32,
    masks: &OceanMasks,
    slot: usize,
) -> [f32; 2] {
    let global = global_spacing.max(1.0);
    let outer = outer_spacing.max(1.0);
    let snapped = [
        (camera[0] / global).round() * global,
        (camera[2] / global).round() * global,
    ];
    let nearest = masks.closest_point(slot, snapped).unwrap_or([
        snapped[0].clamp(surface.minimum[0], surface.maximum[0]),
        snapped[1].clamp(surface.minimum[1], surface.maximum[1]),
    ]);
    [
        (nearest[0] / outer).round() * outer,
        (nearest[1] / outer).round() * outer,
    ]
}

fn rect_intersects_pvs_tiles(
    surface: OceanSurface,
    tiles: &OceanPvsTileMask,
    lo: [f32; 2],
    hi: [f32; 2],
) -> bool {
    if hi[0] < surface.minimum[0]
        || hi[1] < surface.minimum[1]
        || lo[0] > surface.maximum[0]
        || lo[1] > surface.maximum[1]
    {
        return false;
    }
    let size = [
        (surface.maximum[0] - surface.minimum[0]).max(1.0e-4),
        (surface.maximum[1] - surface.minimum[1]).max(1.0e-4),
    ];
    let tile = |value: f32, axis: usize| {
        (((value - surface.minimum[axis]) / size[axis] * OCEAN_PRUNE_TILE_AXIS as f32).floor()
            as i32)
            .clamp(0, OCEAN_PRUNE_TILE_AXIS as i32 - 1) as usize
    };
    let (x0, x1, y0, y1) = (
        tile(lo[0], 0),
        tile(hi[0], 0),
        tile(lo[1], 1),
        tile(hi[1], 1),
    );
    (y0..=y1).any(|y| {
        (x0..=x1).any(|x| tiles[y] & (1u32 << x) != 0)
    })
}

/// Shared Godot/Blue Water conservative cell admission. Only cells proved
/// outside the exact water footprint, visible BSP water tiles and active sky
/// contacts are removed. Boundary cells remain for the fragment mask.
/// `mask_slot == None` retains Godot's unmasked rectangular fallback.
pub(crate) fn promoted_ocean_cell_may_contain(
    surface: OceanSurface,
    masks: &OceanMasks,
    mask_slot: Option<usize>,
    pvs_tiles: Option<&OceanPvsTileMask>,
    sky: Option<&sky_extend::SkyExtension>,
    lo: [f32; 2],
    hi: [f32; 2],
) -> bool {
    let in_bounds = (0..2).all(|axis|
        hi[axis] >= surface.minimum[axis] && lo[axis] <= surface.maximum[axis]);
    // Godot clamps its unmasked mesh to these bounds; Blue Water doesn't.
    let original = in_bounds && mask_slot.is_none_or(|slot| masks.rect_may_contain(slot, lo, hi));
    if let Some(sky) = sky {
        return original || sky.rect_may_contain(lo, hi);
    }
    original && pvs_tiles.is_none_or(|tiles| rect_intersects_pvs_tiles(surface, tiles, lo, hi))
}

#[cfg(test)]
mod native_clip_prune_tests {
    use super::*;

    #[test]
    fn unmasked_rectangle_and_empty_pvs_are_pruned_before_gpu_submission() {
        let source = OceanSurface {
            plane_height: 0.0, minimum: [100.0, 200.0], maximum: [300.0, 400.0],
        };
        let masks = OceanMasks::default();
        assert!(promoted_ocean_cell_may_contain(source, &masks, Some(0), None,
            None, [110.0, 210.0], [120.0, 220.0]));
        assert!(!promoted_ocean_cell_may_contain(source, &masks, Some(0), None,
            None, [-100.0, -100.0], [0.0, 0.0]));
        let pvs_none = [0u32; OCEAN_PRUNE_TILE_AXIS];
        assert!(!promoted_ocean_cell_may_contain(source, &masks, Some(0), Some(&pvs_none),
            None, [110.0, 210.0], [120.0, 220.0]));
        let mut pvs_visible = [0u32; OCEAN_PRUNE_TILE_AXIS];
        pvs_visible[2] = 1u32 << 2;
        assert!(promoted_ocean_cell_may_contain(source, &masks, Some(0), Some(&pvs_visible),
            None, [111.0, 211.0], [122.0, 222.0]));
        assert!(!promoted_ocean_cell_may_contain(source, &masks, Some(0), Some(&pvs_visible),
            None, [270.0, 370.0], [275.0, 375.0]));
    }
}

impl Default for OceanMasks {
    fn default() -> Self {
        Self {
            ranges: [[0.0; 4]; OCEAN_MAX_SURFACES],
            bounds: [[0.0; 4]; OCEAN_MAX_SURFACES],
            tiles: [[0; 4]; OCEAN_MAX_SURFACES],
            prune_tiles: [[0; OCEAN_PRUNE_TILE_WORDS]; OCEAN_MAX_SURFACES],
            triangles: Vec::new(),
            footprints: std::array::from_fn(|_| None),
            footprint_indices: std::array::from_fn(|_| None),
            boundaries: std::array::from_fn(|_| None),
        }
    }
}

/// Outline edges used by exactly one triangle: the shoreline and hole rims. An
/// edge split by a T-junction reads as extra boundary, which can only make the
/// derived clearance smaller, never larger.
fn outline_boundary(triangles: &[MaskTriangle]) -> Vec<[f32; 4]> {
    let key = |p: [f32; 2]| ((p[0] * 8.0).round() as i32, (p[1] * 8.0).round() as i32);
    let mut edges = std::collections::HashMap::<_, (u32, [f32; 4])>::new();
    for t in triangles {
        for i in 0..3 {
            let (a, b) = (t[i], t[(i + 1) % 3]);
            let (ka, kb) = (key(a), key(b));
            if ka == kb {
                continue;
            }
            let entry = edges
                .entry(if ka < kb { (ka, kb) } else { (kb, ka) })
                .or_insert((0, [a[0], a[1], b[0], b[1]]));
            entry.0 += 1;
        }
    }
    edges.into_values().filter(|(uses, _)| *uses == 1).map(|(_, edge)| edge).collect()
}

fn segment_touches_rect(edge: [f32; 4], lo: [f32; 2], hi: [f32; 2]) -> bool {
    let (mut t0, mut t1) = (0.0_f32, 1.0_f32);
    for axis in 0..2 {
        let origin = edge[axis];
        let delta = edge[axis + 2] - origin;
        if delta.abs() < 1.0e-9 {
            if origin < lo[axis] || origin > hi[axis] {
                return false;
            }
            continue;
        }
        let (mut near, mut far) = ((lo[axis] - origin) / delta, (hi[axis] - origin) / delta);
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        t0 = t0.max(near);
        t1 = t1.min(far);
        if t0 > t1 {
            return false;
        }
    }
    true
}

const MASK_TILE_BOUNDARY: u32 = 0;
const MASK_TILE_OUTSIDE: u32 = 1;
const MASK_TILE_INSIDE: u32 = 2;
const MASK_TOLERANCE: f32 = 0.75;

fn mask_triangle_edges(t: MaskTriangle, p: [f32; 2]) -> [f32; 3] {
    let edge = |from: [f32; 2], to: [f32; 2]| {
        let d = [to[0] - from[0], to[1] - from[1]];
        (d[0] * (p[1] - from[1]) - d[1] * (p[0] - from[0]))
            / d[0].hypot(d[1]).max(1.0e-4)
    };
    [edge(t[0], t[1]), edge(t[1], t[2]), edge(t[2], t[0])]
}

fn mask_triangle_contains(t: MaskTriangle, p: [f32; 2]) -> bool {
    let e = mask_triangle_edges(t, p);
    e.iter().all(|&v| v >= -MASK_TOLERANCE)
        || e.iter().all(|&v| v <= MASK_TOLERANCE)
}

fn set_mask_tile(words: &mut [u32], tile: usize, code: u32) {
    let bit = tile * 2;
    words[bit / 32] |= code << (bit % 32);
}

/// A tile the outline (the triangles' union boundary) does not reach, even
/// grown by the mask tolerance, lies wholly on one side of it: inside or
/// outside by its centre. Only tiles the coastline crosses keep the triangle
/// test. Asking whether one triangle holds the whole tile instead left every
/// tile across two triangles' shared edge a boundary tile.
fn build_mask_tiles<const AXIS: usize, const WORDS: usize>(
    triangles: &[MaskTriangle],
    minimum: [f32; 2],
    maximum: [f32; 2],
) -> [u32; WORDS] {
    // An exact duplicate would hide its edges from `outline_boundary`.
    let key = |p: [f32; 2]| ((p[0] * 8.0).round() as i32, (p[1] * 8.0).round() as i32);
    let mut seen = std::collections::HashSet::new();
    let unique: Vec<MaskTriangle> = triangles.iter().copied().filter(|t| {
        let mut corners = t.map(key);
        corners.sort_unstable();
        seen.insert(corners)
    }).collect();
    let outline = outline_boundary(&unique);
    let size = [maximum[0] - minimum[0], maximum[1] - minimum[1]];
    let mut tiles = [0u32; WORDS];
    for y in 0..AXIS {
        for x in 0..AXIS {
            let lo = [
                minimum[0] + size[0] * x as f32 / AXIS as f32,
                minimum[1] + size[1] * y as f32 / AXIS as f32,
            ];
            let hi = [
                minimum[0] + size[0] * (x + 1) as f32 / AXIS as f32,
                minimum[1] + size[1] * (y + 1) as f32 / AXIS as f32,
            ];
            let grown_lo = [lo[0] - MASK_TOLERANCE, lo[1] - MASK_TOLERANCE];
            let grown_hi = [hi[0] + MASK_TOLERANCE, hi[1] + MASK_TOLERANCE];
            let centre = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
            let code = if outline.iter().any(|&edge| segment_touches_rect(edge, grown_lo, grown_hi)) {
                MASK_TILE_BOUNDARY
            } else if unique.iter().any(|&t| mask_triangle_contains(t, centre)) {
                MASK_TILE_INSIDE
            } else {
                MASK_TILE_OUTSIDE
            };
            set_mask_tile(&mut tiles, y * AXIS + x, code);
        }
    }
    tiles
}

fn mask_tile_code(bounds: [f32; 4], words: [u32; 4], p: [f32; 2]) -> u32 {
    if p[0] < bounds[0] || p[1] < bounds[1] || p[0] > bounds[2] || p[1] > bounds[3] {
        return MASK_TILE_OUTSIDE;
    }
    let size = [(bounds[2] - bounds[0]).max(1.0e-4), (bounds[3] - bounds[1]).max(1.0e-4)];
    let x = (((p[0] - bounds[0]) / size[0] * OCEAN_MASK_TILE_AXIS as f32).floor() as i32)
        .clamp(0, OCEAN_MASK_TILE_AXIS as i32 - 1) as usize;
    let y = (((p[1] - bounds[1]) / size[1] * OCEAN_MASK_TILE_AXIS as f32).floor() as i32)
        .clamp(0, OCEAN_MASK_TILE_AXIS as i32 - 1) as usize;
    let bit = (y * OCEAN_MASK_TILE_AXIS + x) * 2;
    (words[bit / 32] >> (bit % 32)) & 3
}

fn packed_tile_code(words: &[u32], tile: usize) -> u32 {
    let bit = tile * 2;
    (words[bit / 32] >> (bit % 32)) & 3
}

/// Whether a promoted surface actually needs the per-fragment outline mask.
///
/// BSP water faces on one plane form a non-overlapping planar tessellation. If
/// their summed triangle area fills the cluster's axis-aligned footprint, the
/// clipmap rectangle already is the exact water silhouette and scanning those
/// triangles in the fragment shader is pure overhead. Irregular, rotated and
/// holed surfaces retain the exact triangle mask.
pub fn promoted_outline_needs_mask(
    minimum: [f32; 2],
    maximum: [f32; 2],
    triangles: &[MaskTriangle],
) -> bool {
    if triangles.is_empty() {
        return false;
    }
    let width = f64::from((maximum[0] - minimum[0]).max(0.0));
    let height = f64::from((maximum[1] - minimum[1]).max(0.0));
    let bounds_area = width * height;
    if bounds_area <= 1.0e-8 {
        return false;
    }
    let triangle_area: f64 = triangles
        .iter()
        .map(|t| {
            let ab = [
                f64::from(t[1][0]) - f64::from(t[0][0]),
                f64::from(t[1][1]) - f64::from(t[0][1]),
            ];
            let ac = [
                f64::from(t[2][0]) - f64::from(t[0][0]),
                f64::from(t[2][1]) - f64::from(t[0][1]),
            ];
            (ab[0] * ac[1] - ab[1] * ac[0]).abs() * 0.5
        })
        .sum();
    // Keep this much tighter than any meaningful map-space hole; f64 is used
    // specifically so a huge sea does not need a huge absolute tolerance.
    let tolerance = (bounds_area * 1.0e-8).max(0.01);
    (triangle_area - bounds_area).abs() > tolerance
}

impl OceanMasks {
    pub fn set_footprint(&mut self,slot:usize,footprint:&WaterFootprint) {
        if slot>=OCEAN_MAX_SURFACES {return;}
        self.footprint_indices[slot]=Some(std::sync::Arc::new(footprint_index::FootprintIndex::new(footprint)));
        self.footprints[slot]=Some(std::sync::Arc::new(footprint.clone()));
    }
    pub fn push_footprint(&mut self, slot: usize, footprint: &WaterFootprint) -> bool {
        if slot >= OCEAN_MAX_SURFACES {
            return false;
        }
        self.set_footprint(slot,footprint);
        self.push_surface(slot, &footprint.triangles)
    }

    /// Stores `triangles` for `slot`. Returns false, leaving the slot unmasked,
    /// when they do not fit; the surface then fills its bounding box.
    pub fn push_surface(&mut self, slot: usize, triangles: &[MaskTriangle]) -> bool {
        if slot >= OCEAN_MAX_SURFACES || triangles.is_empty()
            || (self.triangles.len() / 2 + triangles.len()) * 2 > OCEAN_MASK_VEC4S
        {
            return false;
        }
        let minimum: [f32; 2] = std::array::from_fn(|axis| {
            triangles.iter().flat_map(|t| t.iter()).map(|p| p[axis]).fold(f32::INFINITY, f32::min)
        });
        let maximum: [f32; 2] = std::array::from_fn(|axis| {
            triangles.iter().flat_map(|t| t.iter()).map(|p| p[axis]).fold(f32::NEG_INFINITY, f32::max)
        });
        self.bounds[slot] = [minimum[0], minimum[1], maximum[0], maximum[1]];
        self.tiles[slot] = build_mask_tiles::<OCEAN_MASK_TILE_AXIS, 4>(
            triangles, minimum, maximum,
        );
        self.prune_tiles[slot] = build_mask_tiles::<OCEAN_PRUNE_TILE_AXIS, OCEAN_PRUNE_TILE_WORDS>(
            triangles, minimum, maximum,
        );
        self.ranges[slot] = [(self.triangles.len() / 2) as f32, triangles.len() as f32, 0.0, 0.0];
        self.boundaries[slot] = Some(std::sync::Arc::new(outline_boundary(triangles)));
        for t in triangles {
            self.triangles.push([t[0][0], t[0][1], t[1][0], t[1][1]]);
            self.triangles.push([t[2][0], t[2][1], 0.0, 0.0]);
        }
        true
    }

    /// Whether `slot` carries a real outline. Unmasked slots fill their box.
    pub fn is_masked(&self, slot: usize) -> bool {
        slot < OCEAN_MAX_SURFACES && self.ranges[slot][1] > 0.0
    }

    /// Whether the whole xz rectangle lies on the outline (no rim crosses or
    /// touches it). An unmasked slot fills its box, so the caller's box test
    /// decides; this returns true.
    pub fn rect_inside(&self, slot: usize, lo: [f32; 2], hi: [f32; 2]) -> bool {
        if !self.is_masked(slot) {
            return true;
        }
        let corners = [[lo[0], lo[1]], [hi[0], lo[1]], [lo[0], hi[1]], [hi[0], hi[1]]];
        if !corners.iter().all(|&corner| self.contains(slot, corner)) {
            return false;
        }
        self.boundaries
            .get(slot)
            .and_then(Option::as_deref)
            .is_some_and(|edges| !edges.iter().any(|&edge| segment_touches_rect(edge, lo, hi)))
    }

    /// CPU twin of `ocean_mask_contains` in bsp.wgsl, rain_sim.wgsl and
    /// ocean_optics.wgsl: whether the world xz point `p` lies on the slot's
    /// outline, with the same 0.75 unit growth. Unmasked slots contain everything.
    pub fn contains(&self, slot: usize, p: [f32; 2]) -> bool {
        if !self.is_masked(slot) {
            return true;
        }
        match mask_tile_code(self.bounds[slot], self.tiles[slot], p) {
            MASK_TILE_OUTSIDE => return false,
            MASK_TILE_INSIDE => return true,
            _ => {}
        }
        let first = self.ranges[slot][0] as usize;
        let count = self.ranges[slot][1] as usize;
        for i in first..first + count {
            let (Some(t0), Some(t1)) = (self.triangles.get(i * 2), self.triangles.get(i * 2 + 1))
            else {
                break;
            };
            let (a, b, c) = ([t0[0], t0[1]], [t0[2], t0[3]], [t1[0], t1[1]]);
            if (0..2).any(|axis| {
                p[axis] < a[axis].min(b[axis]).min(c[axis]) - MASK_TOLERANCE
                    || p[axis] > a[axis].max(b[axis]).max(c[axis]) + MASK_TOLERANCE
            }) {
                continue;
            }
            if mask_triangle_contains([a, b, c], p) {
                return true;
            }
        }
        false
    }

    /// Conservative cell/footprint overlap used to remove dry clipmap cells.
    /// Fine-grid boundary tiles remain present for the fragment mask to trim;
    /// only cells covered entirely by proven-dry tiles are omitted.
    pub fn rect_may_contain(&self, slot: usize, lo: [f32; 2], hi: [f32; 2]) -> bool {
        #[cfg(test)]
        footprint_perf_tests::record_query(slot,lo,hi);
        if let Some(index)=self.footprint_indices.get(slot).and_then(Option::as_deref) {
            if !index.rect_may_contain(lo,hi) {return false;}
        } else if let Some(footprint) = self.footprints.get(slot).and_then(Option::as_deref) {
            let intersects_sparse_tile = footprint.tiles.iter().any(|tile| {
                hi[0] >= tile.minimum[0]
                    && hi[1] >= tile.minimum[1]
                    && lo[0] <= tile.maximum[0]
                    && lo[1] <= tile.maximum[1]
            });
            if !intersects_sparse_tile {
                return false;
            }
        }
        if !self.is_masked(slot) {
            return true;
        }
        let bounds = self.bounds[slot];
        if hi[0] < bounds[0] || hi[1] < bounds[1] || lo[0] > bounds[2] || lo[1] > bounds[3] {
            return false;
        }
        let size = [(bounds[2] - bounds[0]).max(1.0e-4), (bounds[3] - bounds[1]).max(1.0e-4)];
        let cell = |value: f32, axis: usize| {
            (((value - bounds[axis]) / size[axis] * OCEAN_PRUNE_TILE_AXIS as f32).floor() as i32)
                .clamp(0, OCEAN_PRUNE_TILE_AXIS as i32 - 1) as usize
        };
        let x0 = cell(lo[0], 0);
        let x1 = cell(hi[0], 0);
        let y0 = cell(lo[1], 1);
        let y1 = cell(hi[1], 1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                if packed_tile_code(&self.prune_tiles[slot], y * OCEAN_PRUNE_TILE_AXIS + x)
                    != MASK_TILE_OUTSIDE
                {
                    return true;
                }
            }
        }
        false
    }

    pub fn closest_point(&self, slot: usize, p: [f32; 2]) -> Option<[f32; 2]> {
        if let Some(index)=self.footprint_indices.get(slot).and_then(Option::as_deref) {
            return index.closest_point(p);
        }
        let footprint = self.footprints.get(slot)?.as_deref()?;
        let mut best = None::<([f32; 2], f32)>;
        for triangle in &footprint.triangles {
            if mask_triangle_contains(*triangle, p) {
                return Some(p);
            }
            for edge in 0..3 {
                let a = triangle[edge];
                let b = triangle[(edge + 1) % 3];
                let ab = [b[0] - a[0], b[1] - a[1]];
                let length_sq = ab[0] * ab[0] + ab[1] * ab[1];
                let t = if length_sq > 1.0e-8 {
                    (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / length_sq)
                        .clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let q = [a[0] + ab[0] * t, a[1] + ab[1] * t];
                let distance_sq = (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2);
                if best.is_none_or(|(_, old)| distance_sq < old) {
                    best = Some((q, distance_sq));
                }
            }
        }
        best.map(|(point, _)| point)
    }
}

/// A box of water the renderer reasons about: optics, rain and the camera's
/// "am I submerged" test. The box is the cluster's bounding rectangle, so a
/// surface with a hole (an island ring) additionally carries its outline slot;
/// without it the hole would count as water.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterVolume {
    pub minimum: [f32; 3],
    pub maximum: [f32; 3],
    /// Index into [`OceanMasks`], only for surfaces that really have an outline.
    pub mask_slot: Option<u8>,
}

impl WaterVolume {
    /// GPU encoding of `mask_slot` for a spare `w` lane: 0 is "no mask", and
    /// slot `n` is `n + 1`, so a zero-filled uniform stays unmasked.
    pub fn mask_lane(&self) -> f32 {
        self.mask_slot.map_or(0.0, |slot| f32::from(slot) + 1.0)
    }

    /// Whether the render-space point is inside this water, not merely its box.
    pub fn contains(&self, p: [f32; 3], masks: &OceanMasks) -> bool {
        (0..3).all(|axis| p[axis] >= self.minimum[axis] && p[axis] <= self.maximum[axis])
            && self.mask_slot.map_or(true, |slot| masks.contains(usize::from(slot), [p[0], p[2]]))
    }

    /// Waves displace only the upper surface. The authored floor stays fixed.
    pub fn contains_displaced(&self, p: [f32; 3], masks: &OceanMasks, surface_offset: f32) -> bool {
        let mut displaced = *self;
        displaced.maximum[1] += surface_offset;
        displaced.contains(p, masks)
    }
}

pub use jka_assets::bsp::bathymetry::{
    build_water_footprints, WaterFace, WaterFootprint,
};
#[cfg(test)]
use jka_assets::bsp::bathymetry::cluster_water_faces;

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
    /// 0 Godot/Unified, 1 native Blue Water, 2 Hybrid, 3 lakes/ponds.
    /// Off is represented by the existing r_ocean enable state.
    pub backend: u8,
    pub lake: LakeSettings,
    /// Original Blue Water depth/fetch/surf model. Independent of the r_coastal shallow-bank solver.
    pub blue_water_coast: bool,
    pub blue_water_coast_shelter: bool,
    /// Diagnostic A/B: enable ocean surface raster draws (Godot, native and hybrid,
    /// including infinite horizon). Turning this off preserves FFT, foam, and
    /// mesh/topology updates but submits no ocean color/depth fragments.
    pub surface_pixels: bool,
    /// Optional conservative native-clipmap screen-space LOD threshold in
    /// projected pixels per cell. 0 = preserve native mesh selection.
    /// Uses existing watertight full rings, not unsafe per-cell decimation.
    pub screen_lod_pixels: f32,
    /// A/B: simplify the centre and rings according to estimated wave
    /// curvature/projected geometry error. Does not change FFT resolution.
    pub adaptive_mesh: u8,
    /// Mode 2 only: larger values allow coarser local tessellation (1 = original).
    pub adaptive_mesh_tolerance: f32,
    /// Debug-only native LOD counters, computed against identical PVS-pruned full mesh.
    pub adaptive_mesh_debug: bool,
    /// Diagnostic native/Hybrid expensive pixel shading rate:
    /// 0 full; 1=2x2; 2=4x4; 3=distance-aware 2x2; 4=distance-aware 4x4;
    /// 5=adaptive 2x2: full shading only on blocks the 2x2 shading cannot reconstruct
    /// (all preserve full-res depth).
    pub pixel_detail_mode: u8,
    /// `r_oceanCostProbe`: Blue Water surface feature stubs (`shader_composer::COST_PROBE_*`) for per-feature GPU
    /// cost A/B. Not archived; 0 is the real shader.
    pub surface_cost_probe: u32,
    /// `r_oceanSkipFadedCascades`: lossless A/B, a pixel skips the field reads of a cascade that has fully faded.
    pub skip_faded_cascades: bool,
    /// `r_oceanWaveStatistics`: lossy A/B. 1: where a pixel's long side has averaged a wave cascade, its slopes are
    /// drawn as their spectral statistics and its derivative reads skipped (Bruneton 2010); 2: also its foam spread.
    pub wave_statistics: u8,
    /// `r_blueWaterSSRTiles`: upstream's reflection tile table, so rays whose screen box holds only sky skip the march
    /// (lossless). Off binds the zeroed table that traces every ray.
    pub blue_water_ssr_tiles: bool,
    /// `r_oceanOpticsReceiverOnly`: with Godot receiver caustics the only capture reader, write just the receiver
    /// output (image-identical). Off runs the full capture (A/B).
    pub optics_receiver_only: bool,
    /// `r_oceanFarShader`: 0 off; 1 the default far-water stubs (`FAR_SHADER_DEFAULT`); above 1 an explicit
    /// `shader_composer` stub mask. Water past the sky walls is drawn with that slimmer shader (A/B, lossy).
    pub far_shader: u32,
    /// Distance in metres where the distance-aware sparse experiment starts.
    pub pixel_detail_distance: f32,
    /// `r_oceanPixelContrast`: mode 5's luminance-curvature share above which a 2x2 block gets full shading.
    pub pixel_detail_contrast: f32,
    /// `r_oceanPixelDebug`: mode 5 tints its full-shaded blocks by reason (not archived).
    pub pixel_detail_debug: bool,
    pub blue_water_waves: BlueWaterWaves,
    /// Hybrid-only: 1 = original Blue Water FFT, 0 = authored Gerstner swell.
    /// Independent of Godot's authored.slosh; Native backend ignores this setting.
    pub hybrid_fft_blend: f32,
    /// A/B: 0 original; 1 compression limit; 2 bounded spilling crests.
    pub hybrid_breaker: u8,
    /// Separate authored swell for Hybrid; never changes Godot map-authored
    /// waves, their amplitude links, or Blue Water's simulation parameters.
    pub hybrid_swell: OceanAuthoring,
    /// `r_oceanType 5` (backend 2 + this): Hybrid with godot-water-shader's
    /// waves, surface colour and persistent foam (vendor/godot-water-shader)
    /// in place of the FFT displacement, water body and foam. Blue Water lights it.
    pub gerstner_water: bool,
    /// `r_gerstnerWaveScale`: the upstream sea this many times larger (Froude-similar).
    pub gerstner_wave_scale: f32,
    /// `r_gerstnerFoam` bits: persistent foam cascades 1 near (15 m), 2 far (30 m); 4 distant (per-pixel
    /// compression foam past the cascades).
    pub gerstner_foam: u8,
    /// `r_gerstnerFoamRate`: persistent foam steps per second per cascade (staggered); 0 every frame, as upstream.
    pub gerstner_foam_rate: f32,
    /// `r_gerstnerWaterColor`: upstream's water `color` (sRGB 0..1; FoamScene 0, 55, 65).
    pub gerstner_water_color: [f32; 3],
    /// `r_gerstnerCrestColor`: the colour crests blend toward (sRGB 0..1; upstream's linear 0, 0.08, 0.02).
    pub gerstner_crest_color: [f32; 3],
    pub blue_water_optics: BlueWaterOptics,
    /// Blue Water scene-bending refraction; independent of legacy optics.
    pub blue_water_refraction: f32,
    /// 0 off, 1 inexpensive screen offset, 2 physical scene ray trace.
    pub blue_water_refraction_mode: u8,
    /// Native geometry independent of the JKA/Godot mesh quality setting.
    pub blue_water_mesh_segments: u32, // 64/128/256; default 64
    pub blue_water_mesh_levels: u8, // 6..9; default 6
    /// 0 exact JKA footprint/PVS/sky, 1 ignore interior triangles/tiles (keep PVS),
    /// 2 disable all Blue Water admission tests (DIAGNOSTIC, draws through land),
    /// 3 exact footprint/PVS with per-tile triangle candidates.
    pub blue_water_boundary_mode: u8,
    /// 0 full native caustics each frame, 1 update only on native FFT steps,
    /// 2 off, 3 Godot optics receiver (no native caustics generation).
    pub blue_water_caustics_mode: u8,
    /// 0 full native, 1 native without motes, 2 no composite,
    /// 3 native only when submerged, 4 legacy composite when submerged.
    pub blue_water_underwater_mode: u8,
    /// Allow legacy Godot clipmap index recomputation/upload while running Blue Water.
    pub blue_water_prepare_godot_mesh: bool,
    /// Blue Water-to-JKA displacement/normal compute bridge (other water consumers).
    /// Disable for A/B only; downstream legacy effects may sample frozen data.
    pub blue_water_adapter: bool,
    /// Gate native screen-space reflection ray tracing (sky reflection remains).
    pub blue_water_ssr: bool,
    /// Hybrid-only choices: preserve the working Godot transmission, caustics,
    /// submerged surface and cheaper refraction without modifying pure Blue Water.
    pub hybrid_refraction_mode: u8,
    /// 0 native, 1 native FFT, 2 off, 3 Godot curvature, 4 procedural animated.
    pub hybrid_caustics_mode: u8,
    pub hybrid_underwater_mode: u8,
    /// Session-only Hybrid waterline diagnostics: 0 off, 1 throttled CPU trace,
    /// 2 colors Godot triangles, 3 draws a GPU-sampled Blue Water wave-height marker.
    /// This does not influence surface selection, tint, wave simulation, or culling.
    pub hybrid_waterline_debug: u8,
    pub hybrid_ssr: bool,
    /// Hybrid quality: 0 original; 1 optimized honoring SSR; 2 optimized sky-only.
    pub hybrid_material: u8,
    pub hybrid_fast_foam: f32,
    pub hybrid_fast_foam_detail: bool,
    /// Hybrid owns its appearance; editing it must not overwrite native Blue Water.
    /// 1x uses the stock Godot fog-distance and native Blue Water calibration.
    pub hybrid_transparency: f32,
    pub hybrid_fog_color: [f32; 3],
    pub hybrid_water_color: [f32; 3],
    /// Blue Water mid-cascade spans 181m, Godot's caustic cascade spans 57m.
    /// Rescale only Hybrid's receiver sampling to recover a finer pattern.
    pub hybrid_caustics_scale: f32,
    pub hybrid_caustics_gain: f32,
    /// Procedural pattern size and deformation speed; Hybrid only.
    pub hybrid_procedural_scale: f32,
    pub hybrid_procedural_speed: f32,
    /// Optional forward-difference receiver: 3 taps rather than 4.
    pub hybrid_caustics_fast: bool,
    /// Fixed specialization parameter for native Blue Water screen-space traces.
    /// Only the surface pipeline rebuilds when this changes.
    pub blue_water_ssr_steps: u32,
    /// 0: JKA's own water interaction only; 1: Blue Water's ship wake field, trails, bow waves and impact rings bound
    /// to the native / Hybrid surface (A/B against 0).
    pub blue_wake: u8,
    /// Native / Hybrid spindrift: wisps torn off breaking crests from a gale's wind (13 m/s), 1 on.
    pub spindrift: u8,
    pub interaction: bool,
    pub interaction_strength: f32,
    /// 0 off, 1 entry droplets, 2 wake spray, 3 droplets + wake,
    /// 4 entry jets, 5 droplets + jets + wake.
    pub interaction_splash_mode: u8,
    pub interaction_debug: bool,
    pub interaction_lifetime: f32,
    pub interaction_fade: f32,
    pub interaction_damping: f32,
    pub interaction_edge_fade: f32,
    pub optics: optics::Settings,
    pub bounds: Option<OceanSurface>,
    pub authored: OceanAuthoring,
    pub wind: AtmosphericWind,
    pub map_size: u32,
    pub mesh_quality: u8, // 0 low, 1 high (upstream MeshQuality)
    pub updates_per_second: f32,
    /// 0 = original rendering, 1 = near two mesh rings at full simulation Hz,
    /// ring 2 at 2/3, outer rings at 1/2. FFT cascade rates are UNCHANGED.
    pub dynamic_update_rates: bool,
    /// Match global windrow transport to completed high-detail FFT frames.
    /// With mesh LOD enabled, medium/far windrow *display* is also cached at
    /// 32/24 Hz. The clipmap still renders every frame.
    pub wind_foam_sync: bool,
    pub water_color: [f32; 3],
    pub foam_color: [f32; 3],
    // Optical A/B tests; all default to the pre-test rendering path.
    pub foam_detail_mode: u8, // 0 keep foam only (normals LOD out), 1 upstream cascade sampling
    pub foam_activity_mode: u8, // 0 existing promoted gates, 1 skip heuristic, 2 skip amplitude, 3 skip both
    pub foam_gain: f32,
    pub color_mode: u8, // 0 existing, 1 ignore authored tint, 2 opaque body, 3 both
    pub foam_brightness: f32,
    pub sun_gain: f32,
    pub foam_debug: u8, // 0 regular, 1 raw foam, 2 gated foam, 3 final foam
    /// Downwind roll speed of the persistent FFT foam as a fraction of wind
    /// speed (Unity port: OceanFoamDriftFraction). 0 pins foam to the texel
    /// where it formed, which makes it repeat at the cascade's tile pitch.
    pub foam_drift: f32,
    pub visual_fixes: u8, // 0 untouched, 1 balanced, 2 Godot-leaning ocean appearance
    pub foam_source_mode: u8, // 0 original Jacobian, 1 mild, 2 strong grow/crest threshold
    /// Bitmask of `links::*`: swell controls (and roughness) that follow the amplitude.
    pub links: u8,
    pub roughness: f32,
    pub normal_strength: f32,
    pub sea_spray: bool,
    pub wind_foam_streaks: bool,
    /// Stop encoding FFT cascades and wind-foam transport while no ocean
    /// surface has been visible recently. The field simply holds its last
    /// state: a frozen wave field nobody can see is indistinguishable from an
    /// animated one nobody can see, and because `elapsed` stops advancing with
    /// it, the waves resume from where they left off instead of jumping.
    pub simulate_when_hidden: bool,
    pub cascades: [CascadeSettings; OCEAN_CASCADES],
}

impl Default for OceanSettings {
    fn default() -> Self {
        Self {
            backend: 0,
            lake: LakeSettings::default(),
            blue_water_coast: false,
            blue_water_coast_shelter: false,
            surface_pixels: true,
            screen_lod_pixels: 0.0,
            adaptive_mesh: 0,
            adaptive_mesh_tolerance: 1.0,
            adaptive_mesh_debug: false,
            pixel_detail_mode: 0,
            surface_cost_probe: 0,
            skip_faded_cascades: false,
            wave_statistics: 0,
            blue_water_ssr_tiles: true,
            optics_receiver_only: true,
            far_shader: 0,
            pixel_detail_distance: 100.0,
            pixel_detail_contrast: 0.1,
            pixel_detail_debug: false,
            blue_water_waves: BlueWaterWaves::default(),
            hybrid_fft_blend: 1.0,
            hybrid_breaker: 0,
            hybrid_swell: OceanAuthoring::default(),
            blue_water_optics: BlueWaterOptics::default(),
            blue_water_refraction: 0.1,
            blue_water_refraction_mode: 2,
            blue_water_mesh_segments: 64,
            blue_water_mesh_levels: 6,
            blue_water_boundary_mode: 3,
            blue_water_caustics_mode: 0,
            blue_water_underwater_mode: 0,
            blue_water_prepare_godot_mesh: false,
            blue_water_adapter: true,
            blue_water_ssr: true,
            hybrid_refraction_mode: 1,
            hybrid_caustics_mode: 3,
            hybrid_procedural_scale: 1.0,
            hybrid_procedural_speed: 1.0,
            hybrid_underwater_mode: 4,
            hybrid_waterline_debug: 0,
            hybrid_ssr: false,
            hybrid_material: 1,
            hybrid_fast_foam: 1.0,
            hybrid_fast_foam_detail: true,
            hybrid_transparency: 1.0,
            hybrid_fog_color: optics::Settings::default().fog_color,
            hybrid_water_color: BlueWaterOptics::default().water_color,
            hybrid_caustics_scale: 3.0,
            hybrid_caustics_gain: 1.5,
            hybrid_caustics_fast: false,
            gerstner_water: false,
            gerstner_wave_scale: 1.0,
            gerstner_foam: 7,
            gerstner_foam_rate: 60.0,
            gerstner_water_color: GERSTNER_WATER_COLOR,
            gerstner_crest_color: GERSTNER_CREST_COLOR,
            blue_water_ssr_steps: 16,
            blue_wake: 0,
            spindrift: 0,
            interaction: false,
            interaction_strength: 1.0,
            interaction_splash_mode: 3,
            interaction_debug: false,
            interaction_lifetime: 2.5,
            interaction_fade: 1.0,
            interaction_damping: 2.0,
            interaction_edge_fade: 81.92,
            optics: optics::Settings::default(),
            bounds: None,
            authored: OceanAuthoring::default(),
            wind: AtmosphericWind::default(),
            map_size: OCEAN_DEFAULT_MAP_SIZE,
            mesh_quality: 1,
            updates_per_second: 48.0,
            dynamic_update_rates: false,
            wind_foam_sync: true,
            // GodotOceanWaves water.gd defaults. Godot converts these from
            // sRGB to linear before the shader; the BSP shader does the same.
            water_color: [0.1, 0.15, 0.18],
            foam_color: [0.73, 0.67, 0.62],
            // Keep every FFT cascade's normals and foam at any distance, and no extra
            // exponential foam fade: far water must not smooth out into a flat mirror.
            foam_detail_mode: 1,
            foam_activity_mode: 0,
            foam_gain: 1.0,
            color_mode: 0,
            foam_brightness: 1.0,
            sun_gain: 1.0,
            foam_debug: 0,
            foam_drift: 0.0,
            visual_fixes: 0,
            foam_source_mode: 0,
            links: links::ALL,
            // Follows the swell amplitude (links::ROUGHNESS); this is the value at
            // the default 128u sea. Upstream's 0.65 is not used, see derive_from_amplitude.
            roughness: 0.2,
            normal_strength: 1.0,
            sea_spray: true,
            simulate_when_hidden: false,
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
    /// Recomputes every linked control from the swell amplitude. Roughness is
    /// skipped when `keep_roughness` (a map shader authored its own).
    pub fn apply_links(&mut self, keep_roughness: bool) {
        self.authored.apply_links(self.links);
        if self.links & links::ROUGHNESS != 0 && !keep_roughness {
            self.roughness = authoring::derive_from_amplitude(self.authored.amplitude).roughness;
        }
    }

    /// Use Hybrid's Godot submerged optics and ray-length fog. Cheapwater
    /// shares the optics while retaining its own animated mesh and normals.
    pub fn uses_godot_underwater(&self) -> bool {
        (self.backend == 2 && self.blue_water_underwater_mode == 4)
            || (self.backend == 3 && self.lake.shader == 1)
    }

    /// Only Hybrid substitutes the Godot material/mesh draw. Cheapwater shades
    /// its existing animated clipmap below water, including its ripple normals.
    pub fn uses_godot_submerged_mesh(&self) -> bool {
        self.backend == 2 && self.uses_godot_underwater()
    }

    /// Absorption-matched world/clipmap culling distance for the active water
    /// backend. Blue Water derives this from its native RGB extinction; Unified
    /// retains its own transparency-distance and user multiplier.
    pub fn underwater_cull_distance(self) -> f32 {
        if matches!(self.backend, 1 | 2) && (self.backend != 2 || self.hybrid_underwater_mode != 4) {
            self.blue_water_optics.absorption_cull_distance_units()
        } else {
            self.optics.cull_distance()
        }
    }

    /// Godot capture caustics only belong to Hybrid's explicit Godot mode.
    /// The native receiver gets the unchanged master strength separately.
    /// Call on settings resolved for GPU, not on the archived UI values.
    pub fn godot_capture_caustics_strength(self) -> f32 {
        match self.backend {
            0 => self.optics.caustics,
            2 if matches!(self.blue_water_caustics_mode, 3 | 4) => self.optics.caustics,
            // The native receiver has its own separate draw. Its legacy
            // optics capture must never evaluate unobserved Godot caustics.
            _ => 0.0,
        }
    }

    /// Resolve experimental Hybrid optics into the existing Blue Water GPU
    /// fields at the render boundary. Pure Blue Water remains independent.
    pub fn resolved_for_gpu(mut self) -> Self {
        if self.backend == 3 {
            // Lakes retain flat-plane classification; Cheapwater shares Hybrid's
            // submerged material/fog while all lake simulation stays disabled.
            self.blue_water_caustics_mode = 2;
            self.blue_water_underwater_mode = 4;
            self.blue_water_adapter = false;
            self.blue_water_ssr = false;
            self.blue_water_prepare_godot_mesh = false;
            self.blue_wake = 0;
            self.spindrift = 0;
            self.sea_spray = false;
            self.wind_foam_streaks = false;
            self.interaction = false;
            self.optics.caustics = 0.0;
            self.optics.transparency = optics::Settings::default().transparency * self.lake.transparency;
            self.optics.fog_color = self.lake.fog_color;
        } else if self.backend == 2 {
            if self.gerstner_water {
                // Its waves have no FFT component and no crest limiter (`hybrid_swell_uniform`).
                self.hybrid_breaker = 0;
                self.hybrid_fft_blend = 0.0;
            }
            self.blue_water_refraction_mode = self.hybrid_refraction_mode;
            self.blue_water_caustics_mode = self.hybrid_caustics_mode;
            self.blue_water_underwater_mode = self.hybrid_underwater_mode;
            self.blue_water_ssr = self.hybrid_ssr;
            if self.hybrid_material > 0 && !self.gerstner_water {
                self.pixel_detail_mode = 0;
                if self.hybrid_material == 2 { self.blue_water_ssr = false; }
            }
            // Both the Blue Water surface above and legacy submerged optics
            // observe Hybrid's own appearance. Preserve each pure backend's
            // saved appearance for switching/A-B.
            self.blue_water_optics.transparency = self.hybrid_transparency;
            self.blue_water_optics.fog_color = self.hybrid_fog_color;
            self.blue_water_optics.water_color = self.hybrid_water_color;
            // Hybrid owns its clarity like its fog color: scale the stock
            // Godot distance, never the saved Godot preset. A murky saved
            // r_oceanTransparency (e.g. 6 units) otherwise absorbs the whole
            // submerged view to black while the native medium looks fine.
            self.optics.transparency = (optics::Settings::default().transparency
                * self.hybrid_transparency).clamp(1.0, 65536.0);
            self.optics.fog_color = self.hybrid_fog_color;
        } else if self.backend == 1 {
            // Old configs may still contain the pre-split hybrid-only modes.
            // Never let them silently turn Godot optics back on in Native.
            if self.blue_water_caustics_mode >= 3 { self.blue_water_caustics_mode = 0; }
            if self.blue_water_underwater_mode == 4 { self.blue_water_underwater_mode = 0; }
        }
        self
    }

    /// `r_oceanType 5`: Hybrid with godot-water-shader's waves and surface.
    pub fn gerstner_active(&self) -> bool {
        self.backend == 2 && self.gerstner_water
    }

    pub fn sanitize(mut self) -> Self {
        self.backend = self.backend.min(3);
        self.hybrid_breaker = self.hybrid_breaker.min(2);
        self.hybrid_material = self.hybrid_material.min(2);
        self.hybrid_fast_foam = if self.hybrid_fast_foam.is_finite() {
            self.hybrid_fast_foam.clamp(0.0, 3.0)
        } else { 1.0 };
        self.lake = self.lake.sanitize();
        self.blue_water_waves = self.blue_water_waves.sanitize();
        self.hybrid_fft_blend = if self.hybrid_fft_blend.is_finite() {
            self.hybrid_fft_blend.clamp(0.0, 1.0)
        } else { 1.0 };
        self.hybrid_swell = self.hybrid_swell.sanitize();
        self.gerstner_wave_scale = if self.gerstner_wave_scale.is_finite() {
            self.gerstner_wave_scale.clamp(0.1, 20.0)
        } else { 1.0 };
        self.gerstner_foam = self.gerstner_foam.min(7);
        self.gerstner_foam_rate = if self.gerstner_foam_rate.is_finite() {
            self.gerstner_foam_rate.clamp(0.0, 60.0)
        } else { 60.0 };
        self.blue_water_optics = self.blue_water_optics.sanitize();
        self.blue_water_refraction = if self.blue_water_refraction.is_finite() {
            self.blue_water_refraction.clamp(0.0, 0.15)
        } else { 0.0 };
        self.blue_water_refraction_mode = self.blue_water_refraction_mode.min(2);
        self.blue_water_mesh_segments = match self.blue_water_mesh_segments { 64 | 128 | 256 => self.blue_water_mesh_segments, _ => 128 };
        self.blue_water_mesh_levels = self.blue_water_mesh_levels.clamp(6, 9);
        self.blue_water_boundary_mode = self.blue_water_boundary_mode.min(3);
        self.blue_water_caustics_mode = self.blue_water_caustics_mode.min(4);
        self.blue_water_underwater_mode = self.blue_water_underwater_mode.min(4);
        self.hybrid_underwater_mode = self.hybrid_underwater_mode.min(4);
        self.hybrid_caustics_mode = self.hybrid_caustics_mode.min(4);
        self.hybrid_refraction_mode = self.hybrid_refraction_mode.min(2);
        fn bounded(v: f32, fallback: f32, min: f32, max: f32) -> f32 {
            if v.is_finite() { v.clamp(min, max) } else { fallback }
        }
        self.hybrid_transparency = bounded(self.hybrid_transparency, 1.0, 0.1, 10.0);
        self.hybrid_caustics_scale = bounded(self.hybrid_caustics_scale, 3.0, 1.0, 6.0);
        self.hybrid_caustics_gain = bounded(self.hybrid_caustics_gain, 1.5, 0.0, 4.0);
        self.hybrid_procedural_scale = bounded(self.hybrid_procedural_scale, 1.0, 0.25, 4.0);
        self.hybrid_procedural_speed = bounded(self.hybrid_procedural_speed, 1.0, 0.0, 3.0);
        for (color, fallback) in [
            (&mut self.hybrid_fog_color, optics::Settings::default().fog_color),
            (&mut self.hybrid_water_color, BlueWaterOptics::default().water_color),
            (&mut self.gerstner_water_color, GERSTNER_WATER_COLOR),
            (&mut self.gerstner_crest_color, GERSTNER_CREST_COLOR),
        ] {
            for (value, default) in color.iter_mut().zip(fallback) {
                *value = bounded(*value, default, 0.0, 1.0);
            }
        }
        self.blue_water_ssr_steps = self.blue_water_ssr_steps.clamp(4, 32);
        self.blue_wake = self.blue_wake.min(1);
        self.spindrift = self.spindrift.min(1);
        self.interaction_strength = if self.interaction_strength.is_finite() { self.interaction_strength.max(0.0) } else { 1.0 };
        self.interaction_splash_mode = self.interaction_splash_mode.min(5);
        self.interaction_lifetime = if self.interaction_lifetime.is_finite() { self.interaction_lifetime.clamp(0.1,60.0) } else { 2.5 };
        self.interaction_fade = if self.interaction_fade.is_finite() { self.interaction_fade.clamp(0.0,self.interaction_lifetime) } else { 1.0 };
        self.interaction_damping = if self.interaction_damping.is_finite() { self.interaction_damping.max(0.0) } else { 2.0 };
        self.interaction_edge_fade = if self.interaction_edge_fade.is_finite() { self.interaction_edge_fade.clamp(0.0,256.0) } else { 81.92 };
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
        self.updates_per_second = if self.updates_per_second.is_finite() {
            self.updates_per_second.clamp(0.0, 60.0)
        } else { 48.0 };
        for value in self.water_color.iter_mut().chain(self.foam_color.iter_mut()) {
            *value = value.clamp(0.0, 1.0);
        }
        self.foam_detail_mode = self.foam_detail_mode.min(1);
        self.foam_activity_mode = self.foam_activity_mode.min(3);
        self.foam_gain = if self.foam_gain.is_finite() { self.foam_gain.clamp(0.0, 16.0) } else { 1.0 };
        self.color_mode = self.color_mode.min(3);
        self.foam_brightness = if self.foam_brightness.is_finite() { self.foam_brightness.clamp(0.0, 8.0) } else { 1.0 };
        self.sun_gain = if self.sun_gain.is_finite() { self.sun_gain.clamp(0.0, 8.0) } else { 1.0 };
        self.foam_debug = self.foam_debug.min(3);
        self.foam_drift = if self.foam_drift.is_finite() { self.foam_drift.clamp(0.0, 2.0) } else { 0.0 };
        self.visual_fixes = self.visual_fixes.min(2);
        self.foam_source_mode = self.foam_source_mode.min(2);
        self.links &= links::ALL;
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
    time: [f32; 4], // x simulation time, yz foam drift this update (metres, texel axes)
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
    pub spray_planes: [[f32; 4]; OCEAN_MAX_SURFACES], // height, promoted activity, mask slot, foam gain
    pub swell: [f32; 4], // render XZ direction, wave number / metre, amplitude metres
    pub swell_motion: [f32; 4], // angular speed, primary chop, FFT chop, seconds
    pub wind_motion: [f32; 4], // render XZ wind m/s, spray multiplier, promoted clipmap outer spacing
    pub authored_bounds: [f32;4],
    pub authored_plane: [f32;4], // bounds enabled, height, manual appearance, promoted view
    pub mask_ranges: [[f32; 4]; OCEAN_MAX_SURFACES], // first triangle, triangle count
    pub mask_bounds: [[f32; 4]; OCEAN_MAX_SURFACES], // min xz, max xz for tile lookup
    pub mask_tiles: [[u32; 4]; OCEAN_MAX_SURFACES], // 2-bit 8x8 classification
    pub mask_triangles: [[f32; 4]; OCEAN_MASK_VEC4S], // per triangle: a.xy b.xy / c.xy
    // Per-promoted-view only: up to four contacted sky-wall spans on each side.
    // Appended to preserve ALL offsets read by spray, coastal and optics shaders.
    pub sky_segments: [[f32; 4]; sky_extend::SEGMENTS_PER_POOL],
    // Append only: older spray/optics shaders consume just the common prefix.
    pub appearance_a: [f32; 4], // detail mode, foam gate, foam gain, promoted seam flatten (0/1)
    pub appearance_b: [f32; 4], // color mode, foam brightness, sun gain, foam debug
    pub appearance_c: [f32; 4], // x visual fixes, y spatial-LOD ready, z near cell spacing, w analytic far radius
    /// Hybrid: x = vertical lift of the Godot mesh near the camera (metres), y = active.
    pub hybrid_eye: [f32; 4],
    /// Submerged Hybrid only: Blue Water's clipmap frames for `vs_ocean_native`.
    pub native_mesh: NativeOceanMesh,
    /// `OceanMasks::prune_tiles`: the outline's 2-bit 32x32 classification, 16 vec4 per slot.
    pub mask_fine_tiles: [[u32; 4]; OCEAN_FINE_TILE_VEC4S * OCEAN_MAX_SURFACES],
}

const OCEAN_FINE_TILE_VEC4S: usize = OCEAN_PRUNE_TILE_WORDS / 4;

/// Submerged Hybrid draws Blue Water's own clipmap mesh with the Godot world
/// shader (`vs_ocean_native` in bsp.wgsl). This is that pool's upstream
/// `Surface` prefix the vertex stage needs to place it exactly as Blue Water does.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct NativeOceanMesh {
    /// Per level: snapped centre x, z (m), cell (m), 1 / cell.
    pub frames: [[f32; 4]; 10],
    /// Segments, levels, clipmap centre x, z (m).
    pub clipmap: [f32; 4],
    /// Finest level drawn, its morph, plane height (units), mask slot.
    pub detail: [f32; 4],
}

struct PromotedRenderSurface {
    descriptor: appearance::PromotedOcean,
    settings: OceanSettings,
    manual: bool,
    buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

/// Upload-on-demand render-space displacement fields for the *mesh* LODs.
/// FFT cascades keep their original synchronized timing. These lower-resolution
/// snapshots avoid re-sampling all 3 FFT textures at every distant vertex in
/// every 1000+ FPS render frame. They cannot reduce global FFT dispatch count.
struct OceanLodSnapshot {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    _bind_group: wgpu::BindGroup,
    foam: wgpu::Buffer,
    size: u32,
}

/// Preserve the fractional 32/24 Hz deadlines instead of quantizing to FPS.
fn ocean_lod_deadline(next: &mut f32, now: f32, hz: f32) -> bool {
    if now + 1e-6 < *next { return false; }
    let period = 1.0 / hz;
    let skipped = ((now - *next) / period).max(0.0).floor() + 1.0;
    *next += skipped * period;
    true
}

pub struct OceanGpu {
    blue_water: Option<blue_water::BlueWaterOcean>,
    lakes: Option<lakes::LakeRenderer>,
    lake_normal: wgpu::TextureView,
    lake_actors: std::sync::Arc<Vec<interaction::ActorSample>>,
    /// Lazily created: only Hybrid classification asks for the real eye-wave height.
    eye_wave: Option<eye_wave::EyeWaveProbe>,
    /// `r_oceanType 5`'s intersection-foam contact fields, once their worker has built them.
    gerstner_contact: Option<gerstner_contact::ContactFields>,
    sky_extend_enabled: bool,
    sky_infinite_enabled: bool,
    sky_extend_regions: [Option<sky_extend::SkyExtension>; OCEAN_MAX_SURFACES],
    promoted_centres: [Option<[f32; 2]>; OCEAN_MAX_SURFACES],
    promoted_far_radii: [Option<f32>; OCEAN_MAX_SURFACES],
    /// Per pool while its submerged Hybrid surface is the native mesh.
    native_meshes: [Option<NativeOceanMesh>; OCEAN_MAX_SURFACES],
    pub coastal: Option<coastal::Gpu>,
    coastal_render: std::sync::Arc<coastal::RenderResources>,
    coastal_key: Option<(usize, u8, bool, u32, u32)>,
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
    lod_snapshots: [OceanLodSnapshot; 2], // medium (32 Hz) and far (24 Hz)
    lod_pipeline: wgpu::ComputePipeline,
    lod_due: [f32; 2],
    lod_valid: [bool; 2],
    last_windrow_update: Option<f32>,
    staged_params: [CascadeGpu; OCEAN_CASCADES],
    /// Which half of the foam buffer each cascade reads next (it writes the other).
    foam_parity: [u32; OCEAN_CASCADES],
    // One phase/dispatch is encoded at a time; FFT scratch is shared by all
    // cascades, so phase order is strictly cascade 2 -> 1 -> 0.
    phase: usize,
    phase_count: usize, // 0 while idle; 15 FFT passes plus optional spectrum regeneration
    phase_spectrum: bool,
    phase_started_at: f32,
    phase_period: f32,
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
    render_layout: wgpu::BindGroupLayout,
    render_displacement_view: wgpu::TextureView,
    render_normal_view: wgpu::TextureView,
    render_sampler: wgpu::Sampler,
    spray_view: wgpu::TextureView,
    promoted_surfaces: Vec<PromotedRenderSurface>,
    /// Shared map-wide weather roof field and its world-space UV transform.
    /// Keep one reference; no additional water mask bake or texture allocation.
    sky_admission: Option<(wgpu::TextureView, [f32; 4])>,
    interaction_view: wgpu::TextureView,
    interaction_uniform: wgpu::Buffer,
    interaction_splash_uniform: wgpu::Buffer,
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
        cache: crate::pipeline_cache::get().as_ref(),
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
        let interaction = interaction::RenderResources::inert(device);
        let coastal = coastal::RenderResources::inert(device);
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
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(&interaction.view) },
                bg_buffer(7, &interaction.uniform),
                bg_buffer(8, &interaction.splash_uniform),
                bg_buffer(9, &coastal.params),
                bg_buffer(10, &coastal.lookup),
                bg_buffer(11, &coastal.surface),
                wgpu::BindGroupEntry {binding:12, resource:wgpu::BindingResource::TextureView(&displacement_view)},
                wgpu::BindGroupEntry {binding:13, resource:wgpu::BindingResource::TextureView(&displacement_view)},
                bg_buffer(14, &windrow),
                bg_buffer(15, &windrow),
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
                wgpu::BindGroupLayoutEntry{binding:6,visibility:wgpu::ShaderStages::VERTEX|wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Texture{sample_type:wgpu::TextureSampleType::Float{filterable:true},view_dimension:wgpu::TextureViewDimension::D2Array,multisampled:false},count:None},
                uniform_layout_entry(7,wgpu::ShaderStages::VERTEX|wgpu::ShaderStages::FRAGMENT),
                uniform_layout_entry(8,wgpu::ShaderStages::VERTEX),
                uniform_layout_entry(9,wgpu::ShaderStages::VERTEX|wgpu::ShaderStages::FRAGMENT),
                wgpu::BindGroupLayoutEntry{binding:10,visibility:wgpu::ShaderStages::VERTEX|wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Buffer{ty:wgpu::BufferBindingType::Storage{read_only:true},has_dynamic_offset:false,min_binding_size:None},count:None},
                wgpu::BindGroupLayoutEntry{binding:11,visibility:wgpu::ShaderStages::VERTEX|wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Buffer{ty:wgpu::BufferBindingType::Storage{read_only:true},has_dynamic_offset:false,min_binding_size:None},count:None},
                // Render-LOD snapshots; sampled only by the vertex stage.
                wgpu::BindGroupLayoutEntry{binding:12,visibility:wgpu::ShaderStages::VERTEX,ty:wgpu::BindingType::Texture{sample_type:wgpu::TextureSampleType::Float{filterable:true},view_dimension:wgpu::TextureViewDimension::D2Array,multisampled:false},count:None},
                wgpu::BindGroupLayoutEntry{binding:13,visibility:wgpu::ShaderStages::VERTEX,ty:wgpu::BindingType::Texture{sample_type:wgpu::TextureSampleType::Float{filterable:true},view_dimension:wgpu::TextureViewDimension::D2Array,multisampled:false},count:None},
                wgpu::BindGroupLayoutEntry{binding:14,visibility:wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Buffer{ty:wgpu::BufferBindingType::Storage{read_only:true},has_dynamic_offset:false,min_binding_size:None},count:None},
                wgpu::BindGroupLayoutEntry{binding:15,visibility:wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Buffer{ty:wgpu::BufferBindingType::Storage{read_only:true},has_dynamic_offset:false,min_binding_size:None},count:None},
            ]
        })
    }

    #[cfg(test)]
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        render_layout: &wgpu::BindGroupLayout,
        spray_albedo_view: &wgpu::TextureView,
        requested_settings: OceanSettings,
    ) -> Self {
        let interaction = interaction::RenderResources::inert(device);
        let lake_normal = blue_water_gpu::lakes::LakePipeline::normal_map(device, queue, &[128, 128, 255, 255], 1, 1);
        Self::new_with_interaction(device, queue, render_layout, spray_albedo_view, requested_settings, &interaction, &lake_normal)
    }

    pub fn new_with_interaction(
        device: &wgpu::Device, queue: &wgpu::Queue, render_layout: &wgpu::BindGroupLayout,
        spray_albedo_view: &wgpu::TextureView, requested_settings: OceanSettings,
        interaction: &interaction::RenderResources,
        lake_normal: &wgpu::TextureView,
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
        // Two halves per cascade set, ping-ponged by `CascadeGpu::foam[3]`: the unpack
        // pass reads its neighbours' previous foam, so it cannot also write in place.
        let foam_buffer = storage("Ocean persistent foam", n*n*OCEAN_CASCADES as u64*4*2);
        let output_desc = |label: &'static str| wgpu::TextureDescriptor {
            label: Some(label), size: wgpu::Extent3d { width:map_size,height:map_size,depth_or_array_layers:OCEAN_CASCADES as u32 },
            mip_level_count:1, sample_count:1, dimension:wgpu::TextureDimension::D2,
            format:wgpu::TextureFormat::Rgba16Float,
            usage:wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING
                | if cfg!(test) { wgpu::TextureUsages::COPY_SRC } else { wgpu::TextureUsages::empty() },
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
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
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
            cache: crate::pipeline_cache::get().as_ref(),
        });
        let windrow_params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ocean wind foam streak parameters"),
            contents: bytemuck::bytes_of(&WindrowGpu::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let render_uniform = make_render_uniform(false,&settings);
        let coastal_render = coastal::RenderResources::inert(device);
        let render_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label:Some("Ocean render uniform"), contents:bytemuck::bytes_of(&render_uniform), usage:wgpu::BufferUsages::UNIFORM|wgpu::BufferUsages::COPY_DST });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { label:Some("Ocean repeat sampler"), address_mode_u:wgpu::AddressMode::Repeat,address_mode_v:wgpu::AddressMode::Repeat,address_mode_w:wgpu::AddressMode::ClampToEdge,mag_filter:wgpu::FilterMode::Linear,min_filter:wgpu::FilterMode::Linear,mipmap_filter:wgpu::MipmapFilterMode::Linear,..Default::default() });
        // Two inexpensive spatially coarser snapshots of the *complete* FFT
        // displacement field, not two independently simulated wave cascades.
        // 1024 FFT => 512x512 medium and 256x256 far; ~7.5 MiB total.
        let lod_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ocean mesh LOD snapshot shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("ocean_lod_snapshot.wgsl").into()),
        });
        let lod_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ocean mesh LOD snapshot compute layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding:0, visibility:wgpu::ShaderStages::COMPUTE,
                    ty:wgpu::BindingType::Texture { sample_type:wgpu::TextureSampleType::Float {filterable:true},view_dimension:wgpu::TextureViewDimension::D2Array,multisampled:false }, count:None },
                wgpu::BindGroupLayoutEntry { binding:1, visibility:wgpu::ShaderStages::COMPUTE,
                    ty:wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),count:None },
                wgpu::BindGroupLayoutEntry { binding:2, visibility:wgpu::ShaderStages::COMPUTE,
                    ty:wgpu::BindingType::StorageTexture { access:wgpu::StorageTextureAccess::WriteOnly,
                        format:wgpu::TextureFormat::Rgba16Float,view_dimension:wgpu::TextureViewDimension::D2Array },count:None },
            ],
        });
        let lod_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Ocean mesh LOD snapshot pipeline layout"),
            bind_group_layouts: &[Some(&lod_layout)], immediate_size: 0,
        });
        let lod_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Ocean mesh LOD snapshot pipeline"), layout:Some(&lod_pipeline_layout),
            module:&lod_shader, entry_point:Some("main"),
            compilation_options:Default::default(), cache:crate::pipeline_cache::get().as_ref(),
        });
        let lod_snapshots: [OceanLodSnapshot; 2] = std::array::from_fn(|i| {
            let size=(map_size >> (i+1)).max(32);
            let texture=device.create_texture(&wgpu::TextureDescriptor {
                label:Some(if i==0 {"Ocean mesh LOD 2 (32 Hz)"} else {"Ocean mesh LOD 3+ (24 Hz)"}),
                size:wgpu::Extent3d {width:size,height:size,depth_or_array_layers:OCEAN_CASCADES as u32},
                mip_level_count:1, sample_count:1, dimension:wgpu::TextureDimension::D2,
                format:wgpu::TextureFormat::Rgba16Float,
                usage:wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats:&[],
            });
            let view=texture.create_view(&wgpu::TextureViewDescriptor {
                dimension:Some(wgpu::TextureViewDimension::D2Array),..Default::default()
            });
            let bind_group=device.create_bind_group(&wgpu::BindGroupDescriptor {
                label:Some("Ocean mesh LOD downsample"),layout:&lod_layout,entries:&[
                    wgpu::BindGroupEntry{binding:0,resource:wgpu::BindingResource::TextureView(&displacement_view)},
                    wgpu::BindGroupEntry{binding:1,resource:wgpu::BindingResource::Sampler(&sampler)},
                    wgpu::BindGroupEntry{binding:2,resource:wgpu::BindingResource::TextureView(&view)},
                ],
            });
            let foam=device.create_buffer(&wgpu::BufferDescriptor {
                label:Some(if i==0 {"Ocean wind foam 32 Hz snapshot"} else {"Ocean wind foam 24 Hz snapshot"}),
                size: u64::from(OCEAN_WINDROW_SIZE) * u64::from(OCEAN_WINDROW_SIZE) * 4,
                usage:wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation:false,
            });
            OceanLodSnapshot{_texture:texture,view,_bind_group:bind_group,foam,size}
        });
        let blue_water = matches!(settings.backend, 1 | 2).then(|| blue_water::BlueWaterOcean::new(device));
        let render_displacement_view = blue_water.as_ref().map_or_else(|| displacement_view.clone(), |blue| blue.displacement_view.clone());
        let render_normal_view = blue_water.as_ref().map_or_else(|| normal_view.clone(), |blue| blue.normal_view.clone());
        let render_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor { label:Some("Ocean render bind group"), layout:render_layout, entries:&[
            wgpu::BindGroupEntry{binding:0,resource:wgpu::BindingResource::TextureView(&render_displacement_view)},
            wgpu::BindGroupEntry{binding:1,resource:wgpu::BindingResource::TextureView(&render_normal_view)},
            wgpu::BindGroupEntry{binding:2,resource:wgpu::BindingResource::Sampler(&sampler)},
            bg_buffer(3,&render_buffer),
            wgpu::BindGroupEntry{binding:4,resource:wgpu::BindingResource::TextureView(spray_albedo_view)},
            bg_buffer(5, &windrow_field_buffer),
            wgpu::BindGroupEntry{binding:6,resource:wgpu::BindingResource::TextureView(&interaction.view)},
            bg_buffer(7, &interaction.uniform),
            bg_buffer(8, &interaction.splash_uniform),
            bg_buffer(9, &coastal_render.params),
            bg_buffer(10, &coastal_render.lookup),
            bg_buffer(11,&coastal_render.surface),
            wgpu::BindGroupEntry{binding:12,resource:wgpu::BindingResource::TextureView(&lod_snapshots[0].view)},
            wgpu::BindGroupEntry{binding:13,resource:wgpu::BindingResource::TextureView(&lod_snapshots[1].view)},
            bg_buffer(14,&lod_snapshots[0].foam),bg_buffer(15,&lod_snapshots[1].foam),
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

        let mut ocean = Self { blue_water, lakes:None, lake_normal:lake_normal.clone(), lake_actors:std::sync::Arc::new(Vec::new()), eye_wave:None, gerstner_contact:None, sky_extend_enabled:false,sky_infinite_enabled:false,sky_extend_regions:[None;OCEAN_MAX_SURFACES],enabled:false,settings,clipmap_center:[0.0;4],environment:OceanEnvironment::default(),surfaces:[OceanSurface{plane_height:0.0,minimum:[0.0;2],maximum:[0.0;2]};OCEAN_MAX_SURFACES],surface_count:0,masks:std::sync::Arc::new(OceanMasks::default()),times:[120.0,120.0+std::f32::consts::PI,120.0+2.0*std::f32::consts::PI],elapsed:0.0,next_update:0.0,lod_snapshots,lod_pipeline,lod_due:[0.0;2],lod_valid:[false;2],last_windrow_update:None,staged_params:[CascadeGpu::zeroed();OCEAN_CASCADES],foam_parity:[0;OCEAN_CASCADES],phase:0,phase_count:0,phase_spectrum:false,phase_started_at:0.0,phase_period:0.0,spectrum_dirty:true,params_buffer,params_upload_buffer,_spectrum_buffer:spectrum_buffer,_butterfly_buffer:butterfly_buffer,_fft_buffer:fft_buffer,_foam_buffer:foam_buffer,_displacement:displacement,_normal:normal,compute_bind_groups,spectrum_pipeline,butterfly_pipeline,modulate_pipeline,fft_pipeline,transpose_pipeline,unpack_pipeline,_windrow_field_buffer:windrow_field_buffer,windrow_bind_group,windrow_pipeline,windrow_params_buffer,windrow_front:0,render_buffer,render_bind_group, render_layout:render_layout.clone(),
            render_displacement_view, render_normal_view,
            render_sampler:sampler, spray_view:spray_albedo_view.clone(), promoted_surfaces:Vec::new(),
            sky_admission: None,
            interaction_view: interaction.view.clone(), interaction_uniform: interaction.uniform.clone(),
            interaction_splash_uniform: interaction.splash_uniform.clone(),
            coastal: None, coastal_render, coastal_key: None, promoted_centres: [None;OCEAN_MAX_SURFACES], promoted_far_radii: [None;OCEAN_MAX_SURFACES], native_meshes: [None;OCEAN_MAX_SURFACES] };
        ocean.upload_params(queue,0.0);
        let mut encoder=device.create_command_encoder(&wgpu::CommandEncoderDescriptor{label:Some("Ocean FFT init")});
        if settings.backend == 0 {
            let mut pass=encoder.begin_compute_pass(&wgpu::ComputePassDescriptor{label:Some("Ocean butterfly init"),timestamp_writes:None});
            pass.set_pipeline(&ocean.butterfly_pipeline);
            pass.set_bind_group(0,&ocean.compute_bind_groups[0],&[]);
            pass.dispatch_workgroups(map_size/128,stages,1);
        }
        queue.submit(Some(encoder.finish()));
        ocean
    }

    pub fn map_size(&self) -> u32 { self.settings.map_size }
    pub fn backend(&self) -> u8 { self.settings.backend }
    pub(crate) fn set_blue_water_shore(&mut self,device:&wgpu::Device,queue:&wgpu::Queue,maps:Option<std::sync::Arc<upstream_coast::Maps>>) {
        if let Some(blue)=self.blue_water.as_mut() {blue.set_shore(device,queue,maps,self.settings.blue_water_coast_shelter);}
    }

    /// `r_oceanType 5`'s contact fields (`upstream_coast::Source::request_contact`); `None` until they are built.
    pub(crate) fn set_gerstner_contact(&mut self, fields: Option<gerstner_contact::ContactFields>) {
        self.gerstner_contact = fields;
    }


    pub fn set_time(&mut self, seconds: f32) {
        // Screenshot A/Bs already freeze the render clock with this variable.
        // The render-thread scene clock must not advance the ocean behind it.
        static FIXED_TIME: std::sync::OnceLock<Option<f32>> = std::sync::OnceLock::new();
        let seconds = FIXED_TIME.get_or_init(|| std::env::var("JKA_FIXED_SHADER_TIME")
            .ok()?.parse::<f32>().ok().filter(|v| v.is_finite())).unwrap_or(seconds);
        if !seconds.is_finite() { return; }
        if (seconds - self.elapsed).abs() > 0.05 {
            self.elapsed = seconds;
            if let Some(blue) = &mut self.blue_water { blue.set_time(seconds); }
            self.times = std::array::from_fn(|i| seconds + 120.0 + i as f32 * std::f32::consts::PI);
            self.next_update = seconds;
            // Abandon only scratch work; each visible cascade is written in
            // one final unpack dispatch, never a partly computed FFT stage.
            self.phase = 0;
            self.phase_count = 0;
            self.lod_valid = [false;2];
            self.lod_due = [seconds;2];
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
        uniform.appearance_c[1] = if self.settings.dynamic_update_rates && self.lod_valid.iter().all(|v| *v) { 1.0 } else { 0.0 };
        uniform.swell_motion[3] = self.elapsed;
        let wind = self.settings.wind.at(self.elapsed);
        uniform.wind_motion[0] = wind[0] / OCEAN_UNITS_PER_METER;
        uniform.wind_motion[1] = -wind[1] / OCEAN_UNITS_PER_METER;
        apply_scene_context(&mut uniform, self.environment, &self.surfaces, self.surface_count);
        uniform.mask_ranges = self.masks.ranges;
        uniform.mask_bounds = self.masks.bounds;
        uniform.mask_tiles = self.masks.tiles;
        for (slot, words) in self.masks.prune_tiles.iter().enumerate() {
            for (index, chunk) in words.chunks_exact(4).enumerate() {
                uniform.mask_fine_tiles[slot * OCEAN_FINE_TILE_VEC4S + index].copy_from_slice(chunk);
            }
        }
        uniform.mask_triangles[..self.masks.triangles.len()].copy_from_slice(&self.masks.triangles);
        queue.write_buffer(&self.render_buffer,0,bytemuck::bytes_of(&uniform));
        for surface in &self.promoted_surfaces {
            let mut local = make_promoted_render_uniform(&uniform, &surface.settings,
                &surface.descriptor, surface.manual, self.promoted_surfaces.len());
            if let Some(centre)=self.promoted_centres[surface.descriptor.index] {
                local.clipmap[0]=centre[0];local.clipmap[1]=centre[1];
            }
            if let Some(mesh)=self.native_meshes[surface.descriptor.index] {
                local.native_mesh=mesh;
            }
            // Keep clipmap.z as the geometric lattice spacing. Store Phase 2's
            // analytic exterior radius in the reserved appearance_c.w slot.
            local.appearance_c[3]=self.promoted_far_radii[surface.descriptor.index].unwrap_or(0.0);
            // Flatten the outer clipmap ring only where the mesh-free infinite
            // ocean continues it. Only promoted views carry a nonzero value:
            // authored oceans stay untouched.
            let has_infinite_seam = self.sky_extend_enabled && self.sky_infinite_enabled
                && self.sky_extend_regions[surface.descriptor.index].is_some()
                && local.appearance_c[3] > 0.0;
            // appearance_a.w is the seam flatten flag (0/1).
            local.appearance_a[3] = promoted_seam_flatten(has_infinite_seam);
            // Hybrid: lift the Godot mesh so it passes through the surface the
            // native mesh draws at the eye (see eye_wave.rs). Metres; y = active.
            if let Some(shift) = self.eye_wave.as_ref()
                .and_then(|probe| probe.latest(surface.descriptor.index))
                .and_then(|sample| sample.mesh_shift())
            {
                local.hybrid_eye = [shift / OCEAN_UNITS_PER_METER, 1.0, 0.0, 0.0];
            }
            if self.sky_extend_enabled {
                if let Some(region) = self.sky_extend_regions[surface.descriptor.index] {
                    local.sky_segments = region.segments;
                    // 1 = existing promoted pool; 2 = promoted pool with a
                    // render-only sky continuation. All pre-existing promoted
                    // shader tests use > 0.5, so they retain their semantics.
                    // The continuation shade can now tell source water from
                    // background water without a second material or draw.
                    local.authored_plane[3] = 2.0;
                    local.mask_ranges[surface.descriptor.index][2] = region.count as f32;
                    // Rectangular original water needs no triangle mask, but
                    // its source AABB is needed for the sky continuation test.
                    local.mask_bounds[surface.descriptor.index] = region.bounds;
                    let spacing = if surface.settings.mesh_quality > 0 {
                        OCEAN_HIGH_MESH_SPACING_UNITS
                    } else { OCEAN_LOW_MESH_SPACING_UNITS };
                    local.wind_motion[3] = promoted_clipmap_outer_spacing(
                        spacing, f32::INFINITY, surface.settings.authored.amplitude);
                }
            }
            queue.write_buffer(&surface.buffer,0,bytemuck::bytes_of(&local));
        }
    }

    /// Runtime A/B switch; never mutates coastal fields or FFT simulation.
    pub fn set_sky_extension(&mut self, queue: &wgpu::Queue, enabled: bool,
        infinite: bool, regions: &[Option<sky_extend::SkyExtension>]) {
        let mut selected = [None; OCEAN_MAX_SURFACES];
        for (index, region) in regions.iter().enumerate().take(OCEAN_MAX_SURFACES) {
            selected[index] = *region;
        }
        if self.sky_extend_enabled == enabled && self.sky_infinite_enabled == infinite
            && self.sky_extend_regions == selected {
            return;
        }
        self.sky_extend_enabled = enabled;
        self.sky_infinite_enabled = infinite;
        self.sky_extend_regions = selected;
        self.write_render_uniform(queue);
    }

    pub fn render_bind_group_for(&self, index: usize) -> &wgpu::BindGroup {
        self.promoted_surfaces
            .iter()
            .find(|s| s.descriptor.index == index)
            .map_or(&self.render_bind_group, |s| &s.bind_group)
    }

    /// The shader must place the mesh at the same centre the CPU used when
    /// pruning indices. Clamping independently to the AABB caused dry-corner
    /// cameras to draw topology generated for a different water location.
    pub fn set_promoted_clipmap_centres(&mut self, queue:&wgpu::Queue,
        centres:[Option<[f32;2]>;OCEAN_MAX_SURFACES],
        radii:[Option<f32>;OCEAN_MAX_SURFACES]) {
        if self.promoted_centres==centres && self.promoted_far_radii==radii {return;}
        self.promoted_centres=centres;
        self.promoted_far_radii=radii;
        self.write_render_uniform(queue);
    }

    /// Centre and square half-extent the geometric clipmap currently covers for
    /// a promoted pool (post-process fog needs it for no-receiver water pixels).
    pub fn promoted_coverage(&self, slot: usize) -> Option<([f32; 2], f32)> {
        Some(((*self.promoted_centres.get(slot)?)?, (*self.promoted_far_radii.get(slot)?)?))
    }

    /// Rebuild only when the map/selection/amplitude changes. Failed selections
    /// are remembered so unsupported pools do not retry every render frame.
    pub fn configure_coastal(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        enabled: bool, pool: u8,
        atlas: Option<&std::sync::Arc<jka_assets::bsp::bathymetry::Atlas>>,
        promoted: &[appearance::PromotedOcean], overrides: &[(usize, OceanSettings)],
    ) {
        let enabled = enabled && self.settings.backend != 3
            && !(self.settings.blue_water_coast && matches!(self.settings.backend,1|2));
        let pointer=atlas.map_or(0,|a|std::sync::Arc::as_ptr(a) as usize);
        if !enabled && self.coastal.is_none() {return;}
        let mut candidates:Vec<_>=promoted.iter().filter(|p|pool==0 || p.index+1==pool as usize).collect();
        let amplitude=|p:&appearance::PromotedOcean|overrides.iter().find(|(slot,_)|*slot==p.index)
            .map_or(p.heuristic.amplitude,|(_,s)|s.authored.amplitude);
        let signature=candidates.iter().fold(0u32,|h,p| {
            let wavelength=overrides.iter().find(|(slot,_)|*slot==p.index)
                .map_or(p.heuristic.wavelength,|(_,s)|s.authored.wavelength);
            h.wrapping_mul(31)^amplitude(p).to_bits()^wavelength.to_bits()
        });
        let key=(pointer,pool,enabled,signature,self.settings.map_size);
        if self.coastal_key==Some(key) {return;}
        self.coastal_key=Some(key);
        self.coastal=None;
        queue.write_buffer(&self.coastal_render.params,16,bytemuck::cast_slice(&[0u32]));
        if !enabled || pointer==0 {return;}
        if pool==0 {
            candidates.sort_by_key(|p| std::cmp::Reverse(atlas.map_or(0,|a| a.grids.iter()
                .filter(|g|g.slot as usize==p.index)
                .flat_map(|g|g.cells.iter().filter(|c|c.class==jka_assets::bsp::bathymetry::Class::Wet
                    && c.depth/p.heuristic.wavelength.max(1.0)<=0.05))
                .count())));
        }
        let mut rejection="no matching promoted pool".to_owned();
        for descriptor in candidates {
            // Use the effective per-pool wavelength for regime eligibility.
            let settings=appearance::surface_settings(self.settings,descriptor,
                overrides.iter().find(|(slot,_)|*slot==descriptor.index).map(|(_,s)|*s));
            let mut effective=descriptor.clone();
            effective.heuristic.wavelength=settings.authored.wavelength;
            match coastal::prepare(atlas.unwrap(),&effective) {
                Ok(prepared)=> {
                    let gpu=coastal::Gpu::new(device,prepared,&self.render_displacement_view,&self.render_sampler,amplitude(descriptor));
                    eprintln!("COASTAL LIVE: {}",gpu.summary);
                    self.coastal_render=std::sync::Arc::clone(&gpu.render);
                    self.coastal=Some(gpu);
                    // Existing promoted groups reference the old field buffers.
                    // The following set_promoted_surfaces rebuilds them once.
                    self.promoted_surfaces.clear();
                    self.render_bind_group=device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label:Some("Ocean render with coastal field"),layout:&self.render_layout,entries:&[
                            wgpu::BindGroupEntry{binding:0,resource:wgpu::BindingResource::TextureView(&self.render_displacement_view)},
                            wgpu::BindGroupEntry{binding:1,resource:wgpu::BindingResource::TextureView(&self.render_normal_view)},
                            wgpu::BindGroupEntry{binding:2,resource:wgpu::BindingResource::Sampler(&self.render_sampler)},
                            bg_buffer(3,&self.render_buffer),
                            wgpu::BindGroupEntry{binding:4,resource:wgpu::BindingResource::TextureView(&self.spray_view)},
                            bg_buffer(5,&self._windrow_field_buffer),
                            wgpu::BindGroupEntry{binding:6,resource:wgpu::BindingResource::TextureView(&self.interaction_view)},
                            bg_buffer(7,&self.interaction_uniform),bg_buffer(8,&self.interaction_splash_uniform),
                            bg_buffer(9,&self.coastal_render.params),bg_buffer(10,&self.coastal_render.lookup),bg_buffer(11,&self.coastal_render.surface),
                            wgpu::BindGroupEntry{binding:12,resource:wgpu::BindingResource::TextureView(&self.lod_snapshots[0].view)},
                            wgpu::BindGroupEntry{binding:13,resource:wgpu::BindingResource::TextureView(&self.lod_snapshots[1].view)},
                            bg_buffer(14,&self.lod_snapshots[0].foam),bg_buffer(15,&self.lod_snapshots[1].foam),
                        ]});
                    return;
                }
                Err(reason)=>rejection=format!("pool {}: {reason}",descriptor.index+1),
            }
        }
        eprintln!("COASTAL LIVE: FFT fallback; {rejection}");
    }

    pub fn encode_coastal(&mut self,queue:&wgpu::Queue,encoder:&mut wgpu::CommandEncoder,delta:f32) {
        let Some(coastal)=self.coastal.as_mut() else {return;};
        let Some(surface)=self.promoted_surfaces.iter().find(|s|s.descriptor.index==coastal.slot) else {return;};
        let mut shared=make_render_uniform(self.enabled,&self.settings);
        shared.swell_motion[3]=self.elapsed;
        let wave=make_promoted_render_uniform(&shared,&surface.settings,&surface.descriptor,surface.manual,self.promoted_surfaces.len());
        coastal.encode(queue,encoder,delta,&wave);
    }

    /// Rebind only when the shared roof field changes (map load/reset).
    pub fn set_sky_admission(&mut self, field: (wgpu::TextureView, [f32; 4])) {
        if self.sky_admission.as_ref() != Some(&field) {
            if let Some(lakes) = self.lakes.as_mut() { lakes.resources = None; }
            self.sky_admission = Some(field);
        }
    }

    pub fn set_promoted_surfaces(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        descriptors: &[appearance::PromotedOcean],
        overrides: &[(usize, OceanSettings)],
    ) {
        let desired_settings = |p: &appearance::PromotedOcean| {
            let local = overrides
                .iter()
                .find(|(index, _)| *index == p.index)
                .map(|(_, s)| *s);
            (
                appearance::surface_settings(self.settings, p, local),
                local.is_some(),
            )
        };
        if self.promoted_surfaces.len() == descriptors.len()
            && self
                .promoted_surfaces
                .iter()
                .zip(descriptors)
                .all(|(s, p)| {
                    let (settings, manual) = desired_settings(p);
                    s.descriptor == *p && s.settings == settings && s.manual == manual
                })
        {
            return;
        }
        let mut previous = std::mem::take(&mut self.promoted_surfaces);
        for descriptor in descriptors {
            let (settings, manual) = desired_settings(descriptor);
            let mut view = if let Some(i) = previous
                .iter()
                .position(|s| s.descriptor.index == descriptor.index)
            {
                previous.swap_remove(i)
            } else {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Promoted ocean render parameters"),
                    size: std::mem::size_of::<OceanRenderUniform>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Promoted ocean shared FFT view"),
                    layout: &self.render_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(
                                &self.render_displacement_view,
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&self.render_normal_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&self.render_sampler),
                        },
                        bg_buffer(3, &buffer),
                        wgpu::BindGroupEntry {
                            binding: 4,
                            resource: wgpu::BindingResource::TextureView(&self.spray_view),
                        },
                        bg_buffer(5, &self._windrow_field_buffer),
                        wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(&self.interaction_view) },
                        bg_buffer(7, &self.interaction_uniform),
                        bg_buffer(8, &self.interaction_splash_uniform),
                        bg_buffer(9, &self.coastal_render.params),
                        bg_buffer(10, &self.coastal_render.lookup),
                        bg_buffer(11, &self.coastal_render.surface),
                        wgpu::BindGroupEntry{binding:12,resource:wgpu::BindingResource::TextureView(&self.lod_snapshots[0].view)},
                        wgpu::BindGroupEntry{binding:13,resource:wgpu::BindingResource::TextureView(&self.lod_snapshots[1].view)},
                        bg_buffer(14,&self.lod_snapshots[0].foam),bg_buffer(15,&self.lod_snapshots[1].foam),
                    ],
                });
                PromotedRenderSurface {
                    descriptor: descriptor.clone(),
                    settings,
                    manual,
                    buffer,
                    bind_group,
                }
            };
            view.descriptor = descriptor.clone();
            view.settings = settings;
            view.manual = manual;
            self.promoted_surfaces.push(view);
        }
        self.write_render_uniform(queue);
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

    /// Queue a command for Blue Water's ship wake; applied in the next native surface prepare (no-op without the
    /// native backend). Commands only accumulate while the wake is switched on.
    pub fn blue_wake_command(&mut self, command: WakeCommand) {
        if self.settings.blue_wake == 0 { return; }
        if let Some(blue) = self.blue_water.as_mut() {
            if blue.wake_pending.len() < 256 { blue.wake_pending.push(command); }
        }
    }

    pub fn set_settings(&mut self, queue:&wgpu::Queue, requested:OceanSettings) {
        let settings=requested.sanitize();
        debug_assert_eq!(settings.map_size,self.settings.map_size,"map-size changes recreate OceanGpu");
        if settings == self.settings { return; }
        let old_updates=self.settings.updates_per_second;
        let change_schedule = settings.updates_per_second != old_updates;
        if settings.dynamic_update_rates && !self.settings.dynamic_update_rates {
            // Do not disturb an in-flight FFT cycle when enabling ring cache.
            // First show original waves, then publish both snapshots atomically.
            self.lod_valid = [false;2];
            self.lod_due = [self.elapsed;2];
        }
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
        if change_schedule {
            // A partially encoded job can be discarded safely: FFT output
            // textures are only published in its final unpack pass.
            self.phase = 0;
            self.phase_count = 0;
            self.next_update = self.elapsed;
        }
        self.spectrum_dirty |= spectrum_changed;
        // Whitecap/foam rates and displacement/normal scales do not require a
        // spectrum rebuild, but do need fresh compute/render parameters.
        // Do not replace the GPU parameters halfway through a five-pass FFT:
        // every phase in that cascade must use the same spectrum/time/foam data.
        // Changed settings are staged at the next simulation update instead.
        if self.phase_count == 0 {
            self.upload_params(queue,0.0);
        }
        self.write_render_uniform(queue);
    }

    /// Swell heading (degrees) the shared FFT (and so its whitecap foam)
    /// travels along. The FFT is shared by every promoted pool, but each pool
    /// draws its own analytic swell from the heading its map shader scrolls in,
    /// so the largest pool's heading is the swell that is on screen. Without
    /// this the FFT follows the global cvar heading and the whitecaps grow
    /// against the visible swell.
    fn fft_heading_degrees(&self) -> f32 {
        let area = |s: &PromotedRenderSurface| {
            let o = s.descriptor.surface;
            (o.maximum[0] - o.minimum[0]).abs() * (o.maximum[1] - o.minimum[1]).abs()
        };
        self.promoted_surfaces.iter()
            .max_by(|a, b| area(a).total_cmp(&area(b)))
            .map_or(self.settings.authored.direction, |s| s.settings.authored.direction)
    }

    fn build_params_selected(&mut self, deltas:[Option<f32>;OCEAN_CASCADES]) -> [CascadeGpu;OCEAN_CASCADES] {
        let mut gpu=self.staged_params;
        for (i, gpu_param) in gpu.iter_mut().enumerate() {
            let Some(delta) = deltas[i] else { continue; };
            let s=self.settings.cascades[i]; self.times[i]+=delta;
            let authored = self.settings.authored;
            let heading = self.fft_heading_degrees();
            let weather = self.settings.wind;
            let fetch=s.fetch_length.max(0.0001)*1000.0;
            let wind=if i == 2 { (weather.speed / OCEAN_UNITS_PER_METER).max(0.01) } else { s.wind_speed.max(0.0001) };
            let direction = if i == 2 { weather.direction } else { heading + s.wind_direction };
            let resolved_wind = weather.at(self.elapsed);
            let wind_strength = (resolved_wind[0].hypot(resolved_wind[1]) / 800.0).clamp(0.0, 2.0);
            let derivative_scale = if i == 2 { s.displacement_scale * authored.wind_chop * wind_strength } else {
                s.displacement_scale * authored.amplitude / 128.0 * authored.slosh
                    / (authored.wavelength / 1400.0) * authored.steepness / 0.65
            };
            // The source of whitecaps is the FFT displacement Jacobian.
            // Restyling an all-zero foam texture in the pixel shader cannot
            // create whitecaps. This CVar tests source generation separately.
            // It is intentionally uniform across all cascades, and changes
            // only foam thresholds/growth -- never the wave spectrum/FFT.
            let foam_source = self.settings.foam_source_mode.max(self.settings.visual_fixes);
            let (whitecap_bias, grow_multiplier) = match foam_source {
                1 => (0.10, 1.3),
                2.. => (0.20, 1.6),
                _ => (0.0, 1.0),
            };
            // Foam texel axes are (world x, -world z) (the shaders sample with
            // world_m * (1,-1)), and the wind is in JKA units/s, so the downwind
            // roll in texel axes is (wind.x, wind.y) / units-per-metre.
            let drift_scale = self.settings.foam_drift * delta / OCEAN_UNITS_PER_METER;
            let drift = [resolved_wind[0] * drift_scale, resolved_wind[1] * drift_scale];
            let foam_read_half = self.foam_parity[i];
            self.foam_parity[i] ^= 1;
            let spectrum_angle = (direction + 180.0).to_radians();
            // The spectrum is baked by the regenerate pass, so a changed
            // heading (e.g. a different pool becoming dominant) must rebake it.
            if self.staged_params[i].spectrum[3] != spectrum_angle { self.spectrum_dirty = true; }
            *gpu_param=CascadeGpu {
                tile:[s.tile_length[0],s.tile_length[1],derivative_scale,0.0],
                spectrum:[jonswap_alpha(wind,fetch),jonswap_peak(wind,fetch),wind,spectrum_angle],
                shape:[DEPTH_METERS,s.swell,s.detail,s.spread],
                foam:[s.whitecap + wind_strength * 0.15 + whitecap_bias,
                    delta*s.foam_amount*7.5*authored.foam*(0.25+wind_strength)*grow_multiplier,
                    delta/authored.foam_lifetime,foam_read_half as f32],
                seed:[authored.seed,authored.seed ^ (i as u32).wrapping_mul(0x9e3779b9),0,0], time:[self.times[i],drift[0],drift[1],0.0]
            };
        }
        self.staged_params = gpu;
        gpu
    }

    fn upload_params(&mut self, queue:&wgpu::Queue, delta:f32) {
        let gpu=self.build_params_selected([Some(delta);OCEAN_CASCADES]);
        queue.write_buffer(&self.params_buffer,0,bytemuck::cast_slice(&gpu));
    }

    fn stage_params(&mut self, queue:&wgpu::Queue, encoder:&mut wgpu::CommandEncoder, delta:f32) {
        self.stage_params_selected(queue, encoder, [Some(delta);OCEAN_CASCADES]);
    }

    fn stage_params_selected(&mut self, queue:&wgpu::Queue, encoder:&mut wgpu::CommandEncoder,
                             deltas:[Option<f32>;OCEAN_CASCADES]) {
        let gpu=self.build_params_selected(deltas);
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
        self.enabled
            && (self.settings.sea_spray
                || (self.settings.interaction && self.settings.interaction_splash_mode > 0)
                || self.promoted_surfaces.iter().any(|s| {
                    s.settings.sea_spray
                        || (s.settings.interaction && s.settings.interaction_splash_mode > 0)
                }))
    }

    pub fn draw_spray<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera_bind_group: &'a wgpu::BindGroup,
        spray_pipeline: Option<&'a wgpu::RenderPipeline>,
    ) {
        if !self.spray_active() || (self.surface_count == 0 && self.promoted_surfaces.is_empty()) {
            return;
        }
        let Some(spray_pipeline) = spray_pipeline else {
            return;
        };
        pass.set_pipeline(spray_pipeline);
        pass.set_bind_group(0, camera_bind_group, &[]);
        // Each promoted emitter binds the very same wave controls as its
        // surface. Share the original particle budget and FFT textures.
        let splash_start = 32_768;
        let splash_end = splash_start
            + interaction::SPLASH_EVENT_CAPACITY * interaction::SPLASH_INSTANCES_PER_EVENT;
        if self.promoted_surfaces.is_empty() {
            pass.set_bind_group(1, &self.render_bind_group, &[]);
            if self.settings.sea_spray {
                pass.draw(0..6, 0..32_768);
            }
            if self.settings.interaction && self.settings.interaction_splash_mode > 0 {
                pass.draw(0..6, splash_start..splash_end);
            }
        } else {
            let particles = 32768 / self.promoted_surfaces.len().max(1) as u32;
            for surface in &self.promoted_surfaces {
                let has_splashes =
                    surface.settings.interaction && surface.settings.interaction_splash_mode > 0;
                if !surface.settings.sea_spray && !has_splashes {
                    continue;
                }
                pass.set_bind_group(1, &surface.bind_group, &[]);
                if surface.settings.sea_spray {
                    pass.draw(0..6, 0..particles);
                }
                if has_splashes {
                    pass.draw(0..6, splash_start..splash_end);
                }
            }
        }
    }

    /// Dispatches the one-thread solve for the rasterized surface height over `eye`
    /// (render space) in promoted pool `slot`. `None` clears the stored sample.
    pub fn encode_eye_wave_probe(
        &mut self, device:&wgpu::Device, queue:&wgpu::Queue, encoder:&mut wgpu::CommandEncoder,
        eye:[f32;3], slot:Option<usize>,
    ) {
        let (Some(blue), Some(slot)) = (self.blue_water.as_ref(), slot) else {
            if let Some(probe) = self.eye_wave.as_mut() { probe.clear(); }
            return;
        };
        let settings = self.promoted_surfaces.iter().find(|s| s.descriptor.index == slot)
            .map_or(&self.settings, |s| &s.settings);
        let mut uniform = make_render_uniform(self.enabled, settings);
        uniform.swell_motion[3] = self.elapsed;
        let params = eye_wave::EyeWaveParams {
            map_scales: uniform.map_scales,
            ocean_info: uniform.ocean_info,
            swell: uniform.swell,
            swell_motion: uniform.swell_motion,
            eye: [eye[0], eye[2], 0.0, 0.0],
        };
        let sampler = &self.render_sampler;
        let probe = self.eye_wave.get_or_insert_with(|| {
            eye_wave::EyeWaveProbe::new(device, &blue.displacement_view, sampler, blue.surface_waves.layout())
        });
        // The mesh's finest ring; the probe reads the waves at that detail.
        const NATIVE_PROBE_SPACING_METRES: f32 = 0.5;
        let native = probe.has_native().then(|| eye_wave::NativeProbeInput {
            swell: hybrid_swell_uniform(settings, self.elapsed),
            breaker_mode: if settings.backend == 2 { settings.hybrid_breaker } else { 0 },
            godot: gerstner_uniform(settings, self.elapsed, [0.0; 4])[0],
            waves_group: blue.waves(slot).group(&blue.field),
            eye_xz_metres: [eye[0] / OCEAN_UNITS_PER_METER, eye[2] / OCEAN_UNITS_PER_METER],
            spacing_metres: NATIVE_PROBE_SPACING_METRES,
        });
        probe.encode(queue, encoder, &params, native, slot);
    }

    /// Latest rasterized-surface height above pool `slot`'s flat plane at the eye, in
    /// JKA units, a few frames old.
    pub fn eye_wave_sample(&self, slot: usize) -> Option<eye_wave::EyeWaveSample> {
        self.eye_wave.as_ref()?.latest(slot)
    }

    pub fn encode_frame(&mut self, queue:&wgpu::Queue, encoder:&mut wgpu::CommandEncoder, delta:f32) {
        if !self.enabled { return; }
        if self.settings.backend == 3 {
            self.elapsed += delta.max(0.0);
            return;
        }
        if let Some(blue) = &mut self.blue_water {
            blue.adapter_enabled = self.settings.blue_water_adapter;
            blue.encode(queue, encoder, delta, self.settings.updates_per_second, self.settings.blue_water_waves,
                hybrid_swell_uniform(&self.settings,self.elapsed+delta.max(0.0)),
                if self.settings.backend == 2 { self.settings.hybrid_breaker } else { 0 });
            self.elapsed += delta.max(0.0);
            self.write_render_uniform(queue);
            return;
        }
        let delta=delta.max(0.0);
        let now=self.elapsed;
        let updates=self.settings.updates_per_second;
        // 0 Hz is the original unthrottled per-render-frame mode.
        let (fine_fft_completed, completed_cycle) =
            self.encode_original_cascades(queue,encoder,now,delta,updates);
        self.elapsed=now+delta;
        // The near two clipmap levels always sample the just-published FFT.
        // Medium and far snapshot jobs are spatial mesh LODs only. They never
        // make the fine/middle/swell FFT components step out of sync.
        let mut updated_lod = [false;2];
        if self.settings.dynamic_update_rates && completed_cycle {
            for i in 0..2 {
                let hz=if i==0 { updates*(2.0/3.0) } else { updates*0.5 };
                if hz <= 0.0 || ocean_lod_deadline(&mut self.lod_due[i],self.elapsed,hz) {
                    let snapshot=&self.lod_snapshots[i];
                    let mut pass=encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label:Some("Ocean mesh LOD displacement snapshot"),timestamp_writes:None,
                    });
                    pass.set_pipeline(&self.lod_pipeline);
                    pass.set_bind_group(0,&snapshot._bind_group,&[]);
                    pass.dispatch_workgroups(snapshot.size.div_ceil(8),snapshot.size.div_ceil(8),OCEAN_CASCADES as u32);
                    updated_lod[i]=true;
                }
            }
        }
        if self.settings.wind_foam_streaks || self.promoted_surfaces.iter().any(|s| s.settings.wind_foam_streaks) {
            // The FFT and clipmap render independently: clipmap vertices are
            // drawn every frame, but the windrow transport only needs to run
            // when its source (finest FFT normal/foam layer) is republished.
            if !self.settings.wind_foam_sync || fine_fft_completed {
                let dt = self.last_windrow_update
                    .map_or(if updates > 0.0 { 1.0/updates } else { delta },
                            |last| (self.elapsed-last).max(0.0));
                self.encode_windrow(queue, encoder, dt);
                self.last_windrow_update=Some(self.elapsed);
            }
        } else {
            // When re-enabled, do not integrate the entire disabled interval.
            self.last_windrow_update=None;
        }
        // Snapshot foam from the published windrow ping-pong half at exactly
        // the same tick as the matching medium/far displacement cache. The
        // transport itself remains global and follows the finest FFT.
        for (i, updated) in updated_lod.iter().enumerate() {
            if *updated {
                let cells=u64::from(OCEAN_WINDROW_SIZE)*u64::from(OCEAN_WINDROW_SIZE);
                encoder.copy_buffer_to_buffer(&self._windrow_field_buffer,
                    self.windrow_front as u64 * cells * 4,
                    &self.lod_snapshots[i].foam,0,cells*4);
                self.lod_valid[i]=true;
            }
        }
        self.write_render_uniform(queue);
    }

    fn encode_original_cascades(&mut self, queue:&wgpu::Queue, encoder:&mut wgpu::CommandEncoder,
                                now:f32, delta:f32, updates:f32) -> (bool,bool) {
        let target=1.0/(updates+1e-10);
        if self.phase_count == 0 && (updates == 0.0 || now >= self.next_update) {
            let update_delta=if updates==0.0 { delta } else { target+(now-self.next_update) };
            self.next_update=now+target;
            self.stage_params(queue,encoder,update_delta);
            self.phase=0;
            self.phase_spectrum=self.spectrum_dirty;
            self.phase_count=OCEAN_FFT_PASSES+usize::from(self.phase_spectrum);
            self.phase_started_at=now;
            self.phase_period=if updates==0.0 { 0.0 } else { target };
            self.spectrum_dirty=false;
        }
        let mut fine_completed=false;
        let mut cycle_completed=false;
        if self.phase_count > 0 {
            let due=ocean_phases_due(now,self.phase_started_at,self.phase_period,self.phase_count);
            let budget=due.saturating_sub(self.phase).min(if updates == 0.0 {
                self.phase_count
            } else {
                OCEAN_MAX_PASSES_PER_FRAME
            });
            for _ in 0..budget {
                if self.phase_spectrum && self.phase==0 {
                    let n=self.settings.map_size;
                    let mut pass=encoder.begin_compute_pass(&wgpu::ComputePassDescriptor{label:Some("Ocean spectrum regenerate"),timestamp_writes:None});
                    pass.set_pipeline(&self.spectrum_pipeline);
                    pass.set_bind_group(0,&self.compute_bind_groups[0],&[]);
                    pass.dispatch_workgroups(n/16,n/16,OCEAN_CASCADES as u32);
                } else {
                    let fft_phase=self.phase-usize::from(self.phase_spectrum);
                    let cascade=OCEAN_CASCADES-1-fft_phase/OCEAN_PASSES_PER_CASCADE;
                    let phase=fft_phase%OCEAN_PASSES_PER_CASCADE;
                    self.encode_fft_phase(encoder,cascade,phase);
                    fine_completed |= cascade==OCEAN_CASCADES-1 && phase==OCEAN_PASSES_PER_CASCADE-1;
                }
                self.phase+=1;
            }
            if self.phase==self.phase_count {
                self.phase=0;
                self.phase_count=0;
                cycle_completed=true;
            }
        }
        (fine_completed,cycle_completed)
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

    fn encode_fft_phase(&self, encoder:&mut wgpu::CommandEncoder, cascade:usize, phase:usize) {
        let n=self.settings.map_size;
        let bind=&self.compute_bind_groups[cascade];
        // FFT stages have storage read-after-write dependencies. The passes
        // remain separate (including across command submissions) and are
        // always encoded in their original upstream order. Since all FFT
        // cascades share one scratch buffer, never interleave cascades.
        let (label, pipeline, dispatch) = match phase {
            0 => ("GodotOceanWaves spectrum modulate", &self.modulate_pipeline, (n/16,n/16,1)),
            1 => ("GodotOceanWaves FFT rows", &self.fft_pipeline, (1,n,4)),
            2 => ("GodotOceanWaves FFT transpose", &self.transpose_pipeline, (n/32,n/32,4)),
            3 => ("GodotOceanWaves FFT columns", &self.fft_pipeline, (1,n,4)),
            4 => ("GodotOceanWaves FFT unpack", &self.unpack_pipeline, (n/16,n/16,1)),
            _ => unreachable!("five FFT phases per cascade"),
        };
        let mut pass=encoder.begin_compute_pass(&wgpu::ComputePassDescriptor{label:Some(label),timestamp_writes:None});
        pass.set_bind_group(0,bind,&[]);
        pass.set_pipeline(pipeline);
        pass.dispatch_workgroups(dispatch.0,dispatch.1,dispatch.2);
    }

}

/// Number of phases that should have been issued by `now`. The first phase
/// starts immediately; remaining phases are paced across one simulation tick.
/// Slower render FPS can require multiple passes in one frame to keep up.
fn ocean_phases_due(now:f32, start:f32, period:f32, count:usize) -> usize {
    if period <= 0.0 { return count; } // 0 UPS means unthrottled simulation
    let fraction=((now-start).max(0.0)/period).min(1.0);
    (1+(fraction*count as f32).floor() as usize).min(count)
}

fn srgb_channel_to_linear(c:f32)->f32 { if c<=0.04045 { c/12.92 } else { ((c+0.055)/1.055).powf(2.4) } }
fn linear_color(rgb:[f32;3])->[f32;4] { [srgb_channel_to_linear(rgb[0]),srgb_channel_to_linear(rgb[1]),srgb_channel_to_linear(rgb[2]),1.0] }
/// Identical analytic wave parameters for the Hybrid native mesh and eye probe.
/// xy: travel direction, z: radians/metre, w: amplitude in metres.
/// xyzw motion: radians/s, horizontal displacement ratio, FFT weight, seconds.
pub(super) fn hybrid_swell_uniform(settings: &OceanSettings, seconds: f32) -> [[f32; 4]; 2] {
    // `r_oceanType 5` replaces both the FFT and the authored swell with its own waves.
    let gerstner = settings.gerstner_active();
    let mix = if gerstner { 0.0 } else if settings.backend == 2 { settings.hybrid_fft_blend.clamp(0.0, 1.0) } else { 1.0 };
    let authored = settings.hybrid_swell;
    // Travel direction follows the FFT swell direction (same angle convention).
    let angle = settings.blue_water_waves.direction.to_radians();
    let frequency = std::f32::consts::TAU * OCEAN_UNITS_PER_METER / authored.wavelength;
    // Godot uses authored.amplitude as the master height for both its FFT
    // and Gerstner swell. Hybrid uses Blue Water's significant wave height
    // for the FFT, so link the analytic swell to that same master slider.
    // Keep the Gerstner amplitude knob as a relative calibration: at the
    // Blue Water default height its pre-existing strength is unchanged.
    // Cache the reference sea state: building it every draw/pool would add
    // unnecessary CPU work to the hot per-frame uniform path.
    static REFERENCE_WAVE_HEIGHT: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    let reference_height = *REFERENCE_WAVE_HEIGHT
        .get_or_init(|| BlueWaterWaves::default().height.max(0.001));
    let wave_height_scale = settings.blue_water_waves.height.max(0.0) / reference_height;
    let amplitude = if gerstner { 0.0 } else {
        authored.amplitude / OCEAN_UNITS_PER_METER * (1.0 - mix) * wave_height_scale
    };
    let horizontal = 0.9 * authored.steepness
        / (authored.amplitude * std::f32::consts::TAU / authored.wavelength).max(0.00001);
    [
        [angle.cos(), -angle.sin(), frequency, amplitude],
        [std::f32::consts::TAU * authored.speed / authored.wavelength, horizontal, mix, seconds],
    ]
}

/// godot-water-shader's water `color` as FoamScene sets it (sRGB 0, 55, 65).
pub const GERSTNER_WATER_COLOR: [f32; 3] = [0.0, 55.0 / 255.0, 65.0 / 255.0];
/// g_wave's crest colour, linear (0, 0.08, 0.02), as sRGB.
pub const GERSTNER_CREST_COLOR: [f32; 3] = [0.0, 0.31330, 0.15170];

/// godot-water-shader's highest crest above the plane (upstream metres; `godot_water.wgsl`'s `JKA_GODOT_CREST`).
pub(super) const GERSTNER_CREST_METRES: f32 = 0.73;

/// The analytic crest height (m) the Hybrid-family vertex stage adds to the FFT: Hybrid's authored swell, or
/// `r_oceanType 5`'s waves at their scale.
pub(super) fn hybrid_analytic_height(settings: &OceanSettings, seconds: f32) -> f32 {
    if settings.gerstner_active() {
        GERSTNER_CREST_METRES * settings.gerstner_wave_scale
    } else {
        hybrid_swell_uniform(settings, seconds)[0][3]
    }
}

/// The analytic swell the adaptive mesh plans its curvature for ([direction x, z, radians/m, amplitude m]): Hybrid's
/// authored swell, or for `r_oceanType 5` its most curved wave (the third: 0.12 m at 2 rad/m, scaled).
pub(super) fn hybrid_mesh_swell(settings: &OceanSettings, seconds: f32) -> [f32; 4] {
    if settings.gerstner_active() {
        let scale = settings.gerstner_wave_scale;
        let direction = glam::Vec2::new(0.5, 1.0).normalize();
        [direction.x, direction.y, 2.0 / scale, 0.12 * scale]
    } else {
        hybrid_swell_uniform(settings, seconds)[0]
    }
}

/// `r_oceanType 5`'s surface uniform (`hybrid_gerstner.wgsl`'s `godot`, `godot_foam` and `godot_color`): [1 on, wave
/// scale, seconds, foam bits], the persistent foam maps' centres [near x, z, far x, z] (m) and the linear water and
/// crest colours.
/// All zero in other modes.
pub(super) fn gerstner_uniform(settings: &OceanSettings, seconds: f32, foam: [f32; 4]) -> [[f32; 4]; 4] {
    if !settings.gerstner_active() { return [[0.0; 4]; 4]; }
    [[1.0, settings.gerstner_wave_scale, seconds, f32::from(settings.gerstner_foam)], foam,
        linear_color(settings.gerstner_water_color), linear_color(settings.gerstner_crest_color)]
}

fn make_render_uniform(enabled:bool,settings:&OceanSettings)->OceanRenderUniform {
    let mut flat;
    let settings = if settings.backend == 3 {
        flat = *settings;
        flat.authored.amplitude = 0.0;
        flat.normal_strength = 0.0;
        flat.blue_water_waves.height = 0.0;
        for cascade in &mut flat.cascades {cascade.displacement_scale=0.0;cascade.normal_scale=0.0;}
        &flat
    } else {settings};
    let mut scales=[[0.0;4];OCEAN_CASCADES];
    for (i,scale) in scales.iter_mut().enumerate() {
        let cascade=settings.cascades[i];
        let spatial = if i == 2 { 1.0 } else { settings.authored.wavelength / 1400.0 };
        let strength = if i == 2 { settings.authored.wind_chop * (settings.wind.speed / 800.0).clamp(0.0, 2.0) } else { settings.authored.amplitude / 128.0 * settings.authored.slosh };
        let displacement = cascade.displacement_scale * strength;
        *scale=[1.0/(cascade.tile_length[0]*spatial),1.0/(cascade.tile_length[1]*spatial),displacement,cascade.normal_scale*strength/spatial];
    }
    let mut uniform = OceanRenderUniform {
        map_scales:scales,
        water_color:linear_color(settings.water_color),
        // Unused alpha carries an amplitude safety gate for all wave foam.
        // The regular density ramp is applied during FFT foam growth via the
        // linked authored.foam setting; do not multiply that ramp twice here.
        foam_color: {
            let mut color = linear_color(settings.foam_color);
            color[3] = f32::from(settings.authored.amplitude > 0.0);
            color
        },
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
        mask_bounds: [[0.0; 4]; OCEAN_MAX_SURFACES],
        mask_tiles: [[0; 4]; OCEAN_MAX_SURFACES],
        mask_triangles: [[0.0; 4]; OCEAN_MASK_VEC4S],
        sky_segments: [[0.0;4]; sky_extend::SEGMENTS_PER_POOL],
        // Shader detail modes are 1 (foam kept) / 2 (full cascades); w is the promoted
        // seam flatten flag, always 0 for the shared (authored) view.
        appearance_a: [settings.foam_detail_mode as f32 + 1.0, settings.foam_activity_mode as f32,
            settings.foam_gain, 0.0],
        // Values 0..3 are the existing foam diagnostic. 8+ is an opt-in
        // surface-geometry inspection palette, not a water height calculation.
        // Reuse this existing float; do not change any shader binding layouts.
        appearance_b: [settings.color_mode as f32, settings.foam_brightness,
            settings.sun_gain, settings.foam_debug as f32
                + if settings.backend == 2 && settings.hybrid_underwater_mode == 4
                    && settings.hybrid_waterline_debug == 2 { 8.0 } else { 0.0 }],
        appearance_c: [settings.visual_fixes as f32, 0.0,
            if settings.mesh_quality > 0 { OCEAN_HIGH_MESH_SPACING_UNITS } else { OCEAN_LOW_MESH_SPACING_UNITS }, 0.0],
        hybrid_eye: [0.0; 4],
        native_mesh: NativeOceanMesh::zeroed(),
        mask_fine_tiles: [[0; 4]; OCEAN_FINE_TILE_VEC4S * OCEAN_MAX_SURFACES],
    };
    if settings.backend != 0 {
        // Temporary JKA surface adapter: sample Blue Water's own 1,024/181/31 m tiles.
        let cascades = blue_water_core::QualityTier::High.cascades();
        for (i, cascade) in cascades.iter().enumerate() {
            uniform.map_scales[i] = [1.0 / cascade.size as f32, -1.0 / cascade.size as f32, 1.0, 1.0];
        }
        uniform.ocean_info[1] = 256.0;
        let swell = hybrid_swell_uniform(settings, 0.0);
        for scale in &mut uniform.map_scales { scale[2] = swell[1][2]; scale[3] = swell[1][2]; }
        uniform.swell = swell[0];
        uniform.swell_motion = swell[1];
        uniform.swell_motion[2] = 1.0; // Blue Water FFT already encodes its own horizontal chop.
        // With no FFT reads there is no adapter scale tag to sample. The
        // analytic-only budget is known exactly on the CPU.
        if settings.backend == 2 && settings.hybrid_breaker > 0 && swell[1][2] <= 0.0 {
            let compression = (swell[0][3] * swell[0][2] * swell[1][1]).abs();
            let budget = if settings.hybrid_breaker == 1 { 0.9 } else { 1.15 };
            uniform.swell_motion[1] *= (budget / compression.max(1e-5)).min(1.0);
        }
        uniform.surface[2] = 0.0;
        uniform.surface[3] = 0.0;
    }
    if settings.backend == 3 {
        // The native adapter setup above must not restore FFT weights for a
        // lake. Its Godot underside has no FFT, swell, foam or brush-derived tint.
        for scale in &mut uniform.map_scales { scale[2] = 0.0; scale[3] = 0.0; }
        uniform.swell = [0.0; 4];
        uniform.swell_motion = [0.0; 4];
        uniform.water_color = [settings.lake.water_color[0] * settings.lake.tint_color[0],
            settings.lake.water_color[1] * settings.lake.tint_color[1],
            settings.lake.water_color[2] * settings.lake.tint_color[2], 1.0];
        uniform.appearance_b[0] = 1.0;
    }
    uniform
}

/// Whether the shader flattens the outermost clipmap ring. Sky extension mode 1
/// has no flat analytic tail, so its outer ring must keep animating.
fn promoted_seam_flatten(has_analytic_tail: bool) -> f32 {
    if has_analytic_tail { 1.0 } else { 0.0 }
}

/// Use the same render view for the water and its spray emitter. Finest wind
/// chop must follow local amplitude too, rather than retaining full ocean size.
fn make_promoted_render_uniform(
    shared: &OceanRenderUniform,
    settings: &OceanSettings,
    descriptor: &appearance::PromotedOcean,
    manual: bool,
    surface_count: usize,
) -> OceanRenderUniform {
    let mut local = make_render_uniform(shared.ocean_info[0] > 0.5, settings);
    let amplitude_scale = settings.authored.amplitude / 128.0;
    if settings.backend == 0 {
        local.map_scales[2][2] *= amplitude_scale;
        local.map_scales[2][3] *= amplitude_scale;
    }
    local.clipmap = shared.clipmap;
    local.swell_motion[3] = shared.swell_motion[3];
    local.sun_direction_intensity = shared.sun_direction_intensity;
    local.sun_color = shared.sun_color;
    local.sky_ambient = shared.sky_ambient;
    // Shared optical comparison controls apply to all promoted pools, including manual overrides.
    // A/B color override must use the user-facing shared color, not the
    // texture-derived per-pool heuristic which normally replaces it.
    if shared.appearance_b[0] == 1.0 || shared.appearance_b[0] >= 2.5 {
        local.water_color = shared.water_color;
    }
    local.appearance_a = shared.appearance_a;
    local.appearance_b = shared.appearance_b;
    local.appearance_c = shared.appearance_c;
    local.appearance_c[2] = if settings.mesh_quality > 0 { OCEAN_HIGH_MESH_SPACING_UNITS } else { OCEAN_LOW_MESH_SPACING_UNITS };
    let base_spacing = if settings.mesh_quality > 0 {
        OCEAN_HIGH_MESH_SPACING_UNITS
    } else {
        OCEAN_LOW_MESH_SPACING_UNITS
    };
    let extent = (descriptor.surface.maximum[0] - descriptor.surface.minimum[0])
        .max(descriptor.surface.maximum[1] - descriptor.surface.minimum[1]);
    let half_cells = promoted_clipmap_half_cells(settings.authored.amplitude);
    let outer_spacing = promoted_clipmap_outer_spacing(
        base_spacing, extent, settings.authored.amplitude,
    );
    local.wind_motion = [
        shared.wind_motion[0],
        shared.wind_motion[1],
        settings.authored.spray,
        outer_spacing,
    ];
    // X/Y are unused for promoted views except Y, which carries the live
    // clipmap half-width. Z marks a manual appearance override, W a promoted
    // view. This keeps seam snapping in lockstep with runtime index rewrites.
    local.authored_plane = [0.0, half_cells as f32, if manual { 1.0 } else { 0.0 }, 1.0];
    local.spray_info[0] = 1.0;
    local.spray_info[1] = (32768 / surface_count.max(1)) as f32;
    let s = descriptor.surface;
    // Promoted clipmap vertices use this render-only bounds field. Keep
    // spray_bounds[0] as the source-water extent for coastal/spray semantics.
    local.authored_bounds = [s.minimum[0], s.minimum[1], s.maximum[0], s.maximum[1]];
    local.spray_bounds[0] = [s.minimum[0], s.minimum[1], s.maximum[0], s.maximum[1]];
    local.spray_planes[0] = [
        s.plane_height,
        amplitude_scale.clamp(0.0, 1.0),
        descriptor.index as f32,
        settings.authored.foam,
    ];
    local.mask_ranges = shared.mask_ranges;
    local.mask_bounds = shared.mask_bounds;
    local.mask_tiles = shared.mask_tiles;
    local.mask_triangles = shared.mask_triangles;
    local
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
    fn cheapwater_underwater_shares_hybrid_optics_without_fft_or_saved_setting_changes() {
        let mut settings = OceanSettings::default();
        settings.backend = 3;
        settings.lake.shader = 1;
        settings.lake.water_color = [0.8, 0.4, 0.2];
        settings.lake.tint_color = [0.5, 1.0, 1.0];
        settings.lake.fog_color = [0.2, 0.3, 0.4];
        settings.lake.transparency = 2.0;
        let saved = settings;
        let resolved = settings.resolved_for_gpu().sanitize();
        assert!(resolved.uses_godot_underwater());
        assert!(!resolved.uses_godot_submerged_mesh(), "Cheapwater must retain its animated mesh");
        assert_eq!(resolved.optics.fog_color, settings.lake.fog_color);
        assert_eq!(resolved.optics.transparency, optics::Settings::default().transparency * 2.0);
        assert!(!resolved.blue_water_adapter && !resolved.interaction);
        assert_eq!(resolved.optics.caustics, 0.0);
        let uniform = make_render_uniform(true, &resolved);
        assert!(uniform.map_scales.iter().all(|s| s[2] == 0.0 && s[3] == 0.0));
        assert_eq!(uniform.swell, [0.0; 4]);
        assert_eq!(uniform.swell_motion, [0.0; 4]);
        assert_eq!(uniform.foam_color[3], 0.0);
        assert_eq!(uniform.water_color, [0.4, 0.4, 0.2, 1.0]);
        assert_eq!(uniform.appearance_b[0], 1.0);
        assert_eq!(settings, saved);
        settings.lake.shader = 0;
        assert!(!settings.resolved_for_gpu().uses_godot_underwater());
        settings.backend = 2;
        settings.hybrid_underwater_mode = 4;
        assert!(settings.resolved_for_gpu().uses_godot_underwater());
        assert!(settings.resolved_for_gpu().uses_godot_submerged_mesh());
        settings.hybrid_underwater_mode = 0;
        assert!(!settings.resolved_for_gpu().uses_godot_underwater());
        settings.backend = 1;
        assert!(!settings.resolved_for_gpu().uses_godot_underwater());
    }

    #[test]
    fn mask_tiles_follow_the_outline_not_single_triangles() {
        // A rectangle split along its diagonal, as BSP water faces are. No tile
        // across the diagonal fits in one triangle, yet all lie inside the water.
        let square: [MaskTriangle; 2] = [
            [[0.0, 0.0], [100.0, 0.0], [100.0, 100.0]],
            [[0.0, 0.0], [100.0, 100.0], [0.0, 100.0]],
        ];
        let tiles = build_mask_tiles::<8, 4>(&square, [-50.0, -50.0], [150.0, 150.0]);
        let code = |x: usize, y: usize| packed_tile_code(&tiles, y * 8 + x);
        // Tiles are 25 units: 2..=5 lie inside the square, clear of its outline.
        for y in 3..5 { for x in 3..5 { assert_eq!(code(x, y), MASK_TILE_INSIDE, "({x},{y})"); } }
        assert_eq!(code(0, 0), MASK_TILE_OUTSIDE);
        assert_eq!(code(7, 3), MASK_TILE_OUTSIDE);
        // The outline itself (x = 0 and x = 100) keeps the exact triangle test.
        assert_eq!(code(2, 4), MASK_TILE_BOUNDARY);
        assert_eq!(code(5, 4), MASK_TILE_BOUNDARY);
        // An exact duplicate face must not hide the outline.
        let doubled = [square[0], square[1], square[0]];
        assert_eq!(build_mask_tiles::<8, 4>(&doubled, [-50.0, -50.0], [150.0, 150.0]), tiles);
    }

    #[test]
    fn hybrid_gerstner_default_is_identical_to_fft_only() {
        let mut settings = OceanSettings::default();
        settings.backend = 2;
        let data = hybrid_swell_uniform(&settings, 5.0);
        assert_eq!(data[0][3], 0.0, "default Gerstner must add no displacement");
        assert_eq!(data[1][2], 1.0, "default FFT contribution must be untouched");
        let uniform = make_render_uniform(true, &settings);
        assert_eq!(uniform.swell[3], 0.0);
        for scale in uniform.map_scales { assert_eq!(scale[2], 1.0); }
    }

    #[test]
    fn hybrid_gerstner_mix_scales_both_wave_systems() {
        let mut settings = OceanSettings::default();
        settings.backend = 2;
        settings.hybrid_fft_blend = 0.5;
        let data = hybrid_swell_uniform(&settings, 10.0);
        assert_eq!(data[1][2], 0.5);
        assert!((data[0][3] - 1.6).abs() < 0.0001);
        assert_eq!(data[1][3], 10.0);
        let uniform = make_render_uniform(true, &settings);
        assert_eq!(uniform.swell[3], data[0][3]);
        for scale in uniform.map_scales { assert_eq!(scale[2], 0.5); }
        settings.hybrid_fft_blend = 0.0;
        let gerstner = hybrid_swell_uniform(&settings, 10.0);
        assert_eq!(gerstner[1][2], 0.0);
        assert!((gerstner[0][3] - 3.2).abs() < 0.0001);
    }

    #[test]
    fn hybrid_master_wave_height_scales_gerstner_and_preserves_mix() {
        let mut settings = OceanSettings::default();
        settings.backend = 2;
        settings.hybrid_fft_blend = 0.5;
        let reference_height = settings.blue_water_waves.height;
        assert!(reference_height > 0.0);
        let baseline = hybrid_swell_uniform(&settings, 5.0);
        assert!((baseline[0][3] - 1.6).abs() < 0.0001);
        assert_eq!(baseline[1][2], 0.5);

        settings.blue_water_waves.height = reference_height * 2.0;
        let doubled = hybrid_swell_uniform(&settings, 5.0);
        assert!((doubled[0][3] - baseline[0][3] * 2.0).abs() < 0.0001);
        assert_eq!(doubled[1][2], baseline[1][2]);
        // Godot adapter's displaced mesh, native surface and eye-height
        // probe all source these same shared uniform parameters.
        assert_eq!(make_render_uniform(true, &settings).swell[3], doubled[0][3]);

        settings.blue_water_waves.height = 0.0;
        let flat = hybrid_swell_uniform(&settings, 5.0);
        assert_eq!(flat[0][3], 0.0, "zero master height must remove Gerstner");
        assert_eq!(flat[1][2], 0.5, "the cross-sea mix must not change");
        assert_eq!(make_render_uniform(true, &settings).swell[3], 0.0);

        // The separate authored Gerstner height still behaves as a multiplier.
        settings.blue_water_waves.height = reference_height;
        settings.hybrid_swell.amplitude *= 1.5;
        let adjusted = hybrid_swell_uniform(&settings, 5.0);
        assert!((adjusted[0][3] - baseline[0][3] * 1.5).abs() < 0.0001);
    }

    #[test]
    fn hybrid_breaker_analytic_only_adapter_keeps_native_budget() {
        let mut settings = OceanSettings::default();
        settings.backend = 2;
        settings.hybrid_fft_blend = 0.0;
        settings.blue_water_waves.height = 55.8;
        settings.hybrid_swell.steepness = 0.8;
        let raw = hybrid_swell_uniform(&settings, 0.0);
        let original = make_render_uniform(true, &settings);
        assert_eq!(original.swell_motion[1], raw[1][1]);
        for (mode, budget) in [(1, 0.9), (2, 1.15)] {
            settings.hybrid_breaker = mode;
            let adapter = make_render_uniform(true, &settings);
            let compression = (adapter.swell[3] * adapter.swell[2] * adapter.swell_motion[1]).abs();
            assert!((compression - budget).abs() < 1e-5);
            assert_eq!(adapter.swell[3], original.swell[3], "limiting must preserve vertical height");
            assert_eq!(hybrid_swell_uniform(&settings, 0.0), raw,
                "native mesh/probe must receive the raw swell and apply their shared GPU limiter once");
        }
    }

    #[test]
    fn hybrid_gerstner_settings_do_not_change_godot_or_native_backends() {
        let mut settings = OceanSettings::default();
        let original = make_render_uniform(true, &settings);
        settings.hybrid_fft_blend = 0.0;
        settings.hybrid_swell.amplitude = 1024.0;
        assert_eq!(original.swell, make_render_uniform(true, &settings).swell);
        assert_eq!(original.map_scales, make_render_uniform(true, &settings).map_scales);
        settings.backend = 1;
        let data = hybrid_swell_uniform(&settings, 10.0);
        assert_eq!(data[0][3], 0.0);
        assert_eq!(data[1][2], 1.0);
    }

    #[test]
    fn promoted_seam_flatten_follows_the_flat_analytic_tail_only() {
        assert_eq!(promoted_seam_flatten(false), 0.0);
        assert_eq!(promoted_seam_flatten(true), 1.0);
    }

    #[test]
    fn ocean_optical_ab_defaults_keep_distance_detail() {
        let s = OceanSettings::default();
        assert_eq!(s.foam_detail_mode, 1);
        assert_eq!(s.foam_activity_mode, 0);
        assert_eq!(s.color_mode, 0);
        assert_eq!(s.foam_debug, 0);
        assert_eq!(s.visual_fixes, 0);
        assert_eq!(s.foam_source_mode, 0);
        let u = make_render_uniform(true, &s);
        assert_eq!(u.appearance_a, [2.0, 0.0, 1.0, 0.0]);
        assert_eq!(u.appearance_b, [0.0, 1.0, 1.0, 0.0]);
        assert_eq!(u.appearance_c, [0.0, 0.0, OCEAN_HIGH_MESH_SPACING_UNITS, 0.0]);
    }

    #[test]
    fn ocean_optical_ab_reaches_promoted_uniform_without_fft_changes() {
        let mut shared = OceanSettings::default();
        let baseline = make_render_uniform(true, &shared);
        shared.foam_detail_mode = 1;
        shared.foam_activity_mode = 3;
        shared.foam_gain = 8.0;
        shared.color_mode = 3;
        shared.foam_brightness = 1.7;
        shared.sun_gain = 2.0;
        shared.foam_debug = 1;
        shared.visual_fixes = 2;
        shared.foam_source_mode = 1;
        shared.water_color = [0.01, 0.5, 0.95];
        let edited = make_render_uniform(true, &shared);
        assert_eq!(edited.map_scales, baseline.map_scales);
        assert_eq!(edited.appearance_a, [2.0, 3.0, 8.0, 0.0]);
        assert_eq!(edited.appearance_b, [3.0, 1.7, 2.0, 1.0]);
        assert_eq!(edited.appearance_c, [2.0, 0.0, OCEAN_HIGH_MESH_SPACING_UNITS, 0.0]);
        let pool = appearance::tests::pool(0,0.0);
        let local = pool.settings(shared);
        let promoted = make_promoted_render_uniform(&edited, &local, &pool, false, 1);
        assert_eq!(promoted.appearance_a, edited.appearance_a);
        assert_eq!(promoted.appearance_b, edited.appearance_b);
        assert_eq!(promoted.appearance_c, edited.appearance_c);
        assert_eq!(promoted.water_color, edited.water_color);
    }

    #[test]
    fn ocean_optical_ab_sanitizes_invalid_values() {
        let mut s = OceanSettings::default();
        s.foam_detail_mode = 255;
        s.foam_activity_mode = 255;
        s.foam_gain = f32::NAN;
        s.color_mode = 255;
        s.foam_brightness = f32::INFINITY;
        s.sun_gain = f32::NEG_INFINITY;
        s.foam_debug = 255;
        s.visual_fixes = 255;
        s.foam_source_mode = 255;
        let s = s.sanitize();
        assert_eq!(s.foam_detail_mode, 1);
        assert_eq!(s.foam_activity_mode, 3);
        assert_eq!(s.foam_gain, 1.0);
        assert_eq!(s.color_mode, 3);
        assert_eq!(s.foam_brightness, 1.0);
        assert_eq!(s.sun_gain, 1.0);
        assert_eq!(s.foam_debug, 3);
        assert_eq!(s.visual_fixes, 2);
        assert_eq!(s.foam_source_mode, 2);
    }

    #[test]
    fn promoted_water_amplitude_controls_all_displacement_and_spray_views() {
        let shared = OceanSettings::default();
        let mut context = make_render_uniform(true,&shared);
        context.swell_motion[3] = 37.0;
        context.mask_ranges[3] = [2.0,1.0,0.0,0.0];
        let pool = appearance::tests::pool(3,0.0);
        let mut settings = pool.settings(shared);
        // A little authored swell (cross-sea blend < 1) so the swell terms are non-zero.
        settings.authored.slosh = appearance::DEFAULT_PROMOTED_FFT_BLEND;
        let calm = make_promoted_render_uniform(&context,&settings,&pool,false,2);
        assert!(calm.swell[3] > 0.0 && calm.map_scales[0][2] > 0.0);
        settings.authored.amplitude *= 2.0;
        let larger = make_promoted_render_uniform(&context,&settings,&pool,true,2);
        assert_eq!(larger.swell[3],calm.swell[3]*2.0);
        for i in 0..3 {
            assert_eq!(larger.map_scales[i][2],calm.map_scales[i][2]*2.0,"cascade {i} height");
            assert_eq!(larger.map_scales[i][3],calm.map_scales[i][3]*2.0,"cascade {i} normals");
        }
        assert_eq!(larger.spray_planes[0][1],calm.spray_planes[0][1]*2.0);
        assert_eq!(larger.spray_planes[0][2],3.0);
        assert_eq!(larger.spray_bounds[0],[0.0,0.0,1024.0,1024.0]);
        assert_eq!(larger.mask_ranges[3],context.mask_ranges[3]);
        assert_eq!(larger.spray_info[1],16384.0);
        assert_eq!(larger.swell_motion[3],37.0);
    }

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

    /// Water ringing a dry island: four slabs around a 40x40 hole in a 100x100 box.
    fn donut_masks() -> OceanMasks {
        let quad = |x0: f32, y0: f32, x1: f32, y1: f32| {
            [[[x0, y0], [x1, y0], [x1, y1]], [[x0, y0], [x1, y1], [x0, y1]]]
        };
        let triangles: Vec<MaskTriangle> = [
            quad(0.0, 0.0, 100.0, 30.0),
            quad(0.0, 70.0, 100.0, 100.0),
            quad(0.0, 30.0, 30.0, 70.0),
            quad(70.0, 30.0, 100.0, 70.0),
        ]
        .into_iter()
        .flatten()
        .collect();
        let mut masks = OceanMasks::default();
        assert!(masks.push_surface(2, &triangles));
        masks
    }

    #[test]
    fn water_volume_excludes_the_hollow_of_a_masked_surface() {
        let masks = donut_masks();
        let ring = WaterVolume {
            minimum: [0.0, -65536.0, 0.0],
            maximum: [100.0, 50.0, 100.0],
            mask_slot: Some(2),
        };
        assert_eq!(ring.mask_lane(), 3.0);
        // In the ring, below the surface: submerged.
        assert!(ring.contains([10.0, 0.0, 50.0], &masks));
        assert!(ring.contains([50.0, 0.0, 10.0], &masks));
        // Dead centre of the hollow is inside the box but not in the water.
        assert!(!ring.contains([50.0, 0.0, 50.0], &masks));
        // Above the surface and outside the box are dry whatever the mask says.
        assert!(!ring.contains([10.0, 60.0, 50.0], &masks));
        assert!(!ring.contains([150.0, 0.0, 50.0], &masks));
        // Without an outline slot the box is the water, and a zeroed lane is unmasked.
        let plain = WaterVolume { mask_slot: None, ..ring };
        assert_eq!(plain.mask_lane(), 0.0);
        assert!(plain.contains([50.0, 0.0, 50.0], &masks));
        // A slot that was never pushed contains everything.
        assert!(masks.contains(5, [50.0, 50.0]));
        assert!(!masks.is_masked(5));
    }

    #[test]
    fn outline_rect_containment_respects_the_hollow() {
        let masks = donut_masks();
        // Rects: inside one slab, across the hollow rim, wholly in the hollow.
        assert!(masks.rect_inside(2, [5.0, 5.0], [95.0, 25.0]));
        assert!(!masks.rect_inside(2, [5.0, 5.0], [95.0, 45.0]));
        assert!(!masks.rect_inside(2, [45.0, 45.0], [55.0, 55.0]));
        // A rect enclosing the whole hollow has all corners wet but the rim inside it.
        assert!(!masks.rect_inside(2, [10.0, 10.0], [90.0, 90.0]));
        assert!(masks.rect_inside(7, [0.0, 0.0], [1.0, 1.0]));
    }

    #[test]
    fn outline_tiles_are_exact_and_accelerate_interior_and_exterior() {
        let masks = donut_masks();
        let exact = |p: [f32; 2]| {
            let first = masks.ranges[2][0] as usize;
            let count = masks.ranges[2][1] as usize;
            (first..first + count).any(|i| {
                let a = masks.triangles[i * 2];
                let b = masks.triangles[i * 2 + 1];
                mask_triangle_contains(
                    [[a[0], a[1]], [a[2], a[3]], [b[0], b[1]]],
                    p,
                )
            })
        };
        let mut outside = 0;
        let mut inside = 0;
        for y in 0..OCEAN_MASK_TILE_AXIS {
            for x in 0..OCEAN_MASK_TILE_AXIS {
                let p = [(x as f32 + 0.5) * 12.5, (y as f32 + 0.5) * 12.5];
                match mask_tile_code(masks.bounds[2], masks.tiles[2], p) {
                    MASK_TILE_OUTSIDE => outside += 1,
                    MASK_TILE_INSIDE => inside += 1,
                    _ => {}
                }
                assert_eq!(masks.contains(2, p), exact(p), "tile sample {x},{y}");
            }
        }
        for y in 0..=200 {
            for x in 0..=200 {
                let p = [x as f32 * 0.5, y as f32 * 0.5];
                assert_eq!(masks.contains(2, p), exact(p), "dense sample {x},{y}");
            }
        }
        assert!(outside > 0, "the island hollow should have fast dry tiles");
        assert!(inside > 0, "the water slabs should have fast wet tiles");
        assert!(!masks.rect_may_contain(2, [45.0, 45.0], [55.0, 55.0]));
        assert!(masks.rect_may_contain(2, [5.0, 45.0], [15.0, 55.0]));
    }

    #[test]
    fn promoted_clipmap_prunes_dry_cells_only_when_its_snapped_centre_moves() {
        let masks = donut_masks();
        let surface = OceanSurface {
            plane_height: 0.0,
            minimum: [0.0, 0.0],
            maximum: [100.0, 100.0],
        };
        let runtime = build_runtime_promoted_clipmap(1.0, 0.0, surface.minimum,
            surface.maximum, Some(2), appearance::MIN_PROMOTED_AMPLITUDE);
        let half = promoted_clipmap_half_cells(appearance::MIN_PROMOTED_AMPLITUDE);
        let full = runtime.layout.indices_for(half, 0);
        let centre = promoted_clipmap_centre(
            [50.0, 0.0, 50.0],
            surface,
            32.0,
            runtime.layout.outer_spacing_for_half_cells(half),
        );
        let pruned = runtime.layout.indices_for_footprint(
            half, 0, centre, surface, &masks, Some(2), None,
        );
        assert!(pruned.len() < full.len(), "the dry island must remove draw cells");
        assert!(!pruned.is_empty(), "the surrounding water ring remains drawable");

        let same_snap = promoted_clipmap_centre(
            [51.0, 0.0, 51.0],
            surface,
            32.0,
            runtime.layout.outer_spacing_for_half_cells(half),
        );
        assert_eq!(centre, same_snap, "sub-lattice camera motion needs no rewrite");
    }

    #[test]
    fn promoted_clipmap_centres_on_real_l_shaped_water() {
        let rectangle = |lo: [f32; 2], hi: [f32; 2]| {
            vec![
                [lo, [hi[0], lo[1]], [lo[0], hi[1]]],
                [[hi[0], lo[1]], hi, [lo[0], hi[1]]],
            ]
        };
        let faces = [
            WaterFace { plane: 0.0, minimum: [0.0, 0.0], maximum: [100.0, 20.0], authored_ocean: None, material: 1 },
            WaterFace { plane: 0.0, minimum: [0.0, 20.0], maximum: [20.0, 100.0], authored_ocean: None, material: 1 },
        ];
        let triangles = [rectangle([0.0, 0.0], [100.0, 20.0]), rectangle([0.0, 20.0], [20.0, 100.0])];
        let footprint = build_water_footprints(&faces, &triangles, 32.0).remove(0);
        let mut masks = OceanMasks::default();
        assert!(masks.push_footprint(0, &footprint));
        let surface = OceanSurface { plane_height: 0.0, minimum: footprint.minimum, maximum: footprint.maximum };
        let camera = [80.0, 0.0, 80.0];
        assert_eq!(promoted_surface_distance(camera, surface), 0.0);
        assert!(promoted_footprint_distance(camera, surface, &masks, 0) > 59.0);
        let centre = promoted_footprint_clipmap_centre(camera, surface, 1.0, 1.0, &masks, 0);
        assert!(centre[0] <= 21.0 || centre[1] <= 21.0, "centre {centre:?} must lie on an L arm");
    }

    #[test]
    fn promoted_clipmap_prunes_to_the_current_full_pvs_pieces() {
        let surface = OceanSurface {
            plane_height: 0.0,
            minimum: [0.0, 0.0],
            maximum: [100.0, 100.0],
        };
        let rectangle = |lo: [f32; 2], hi: [f32; 2]| {
            vec![
                [lo, [hi[0], lo[1]], [lo[0], hi[1]]],
                [[hi[0], lo[1]], hi, [lo[0], hi[1]]],
            ]
        };
        let mut pvs = OceanPvsFootprints::with_batch_count(2);
        pvs.add_piece(0, 0, surface, rectangle([0.0, 0.0], [40.0, 100.0]));
        pvs.add_piece(1, 0, surface, rectangle([60.0, 0.0], [100.0, 100.0]));

        let left_only = pvs.visible_tiles(&[0]);
        let left = left_only[0].as_ref().expect("surface has exact PVS pieces");
        assert!(rect_intersects_pvs_tiles(surface, left, [5.0, 45.0], [15.0, 55.0]));
        assert!(!rect_intersects_pvs_tiles(surface, left, [85.0, 45.0], [95.0, 55.0]));
        let both = pvs.visible_tiles(&[0, 1]);
        assert!(rect_intersects_pvs_tiles(
            surface,
            both[0].as_ref().unwrap(),
            [85.0, 45.0],
            [95.0, 55.0],
        ));
        let neither = pvs.visible_tiles(&[]);
        assert!(!rect_intersects_pvs_tiles(
            surface,
            neither[0].as_ref().unwrap(),
            [5.0, 45.0],
            [15.0, 55.0],
        ));

        let runtime = build_runtime_promoted_clipmap(
            1.0,
            0.0,
            surface.minimum,
            surface.maximum,
            None,
            appearance::MIN_PROMOTED_AMPLITUDE,
        );
        let half = promoted_clipmap_half_cells(appearance::MIN_PROMOTED_AMPLITUDE);
        let centre = promoted_clipmap_centre(
            [50.0, 0.0, 50.0],
            surface,
            32.0,
            runtime.layout.outer_spacing_for_half_cells(half),
        );
        let full = runtime.layout.indices_for_footprint(
            half,
            0,
            centre,
            surface,
            &OceanMasks::default(),
            None,
            None,
        );
        let visible = runtime.layout.indices_for_footprint(
            half,
            0,
            centre,
            surface,
            &OceanMasks::default(),
            None,
            Some(left),
        );
        assert!(!visible.is_empty());
        assert!(visible.len() < full.len());
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
    fn calm_promoted_water_shrinks_lod_rings_without_losing_coverage() {
        assert_eq!(promoted_clipmap_half_cells(appearance::MIN_PROMOTED_AMPLITUDE), 32);
        assert_eq!(promoted_clipmap_half_cells(PROMOTED_CLIPMAP_FULL_DETAIL_AMPLITUDE), 128);
        assert!(promoted_clipmap_half_cells(16.0) > 32);
        assert!(promoted_clipmap_half_cells(16.0) < 128);

        let minimum = [0.0; 2];
        let maximum = [40_000.0; 2];
        let reference = build_clipmap(10.0, 0.0, minimum, maximum, None);
        let calm = build_promoted_clipmap(
            10.0,
            0.0,
            minimum,
            maximum,
            None,
            appearance::MIN_PROMOTED_AMPLITUDE,
        );
        // 32-cell rings need two extra coarse levels to retain the old map
        // reach, but still submit dramatically less geometry than six 128-cell
        // levels. This is the actual FPS win the calm-water profile is for.
        assert!(calm.indices.len() * 8 < reference.indices.len());
        let step = calm.vertices[0].color[2];
        assert!(reach(&calm) >= 40_000.0 + step * 0.5);

        // By 64 units of displacement promotion is back on the original mesh
        // profile, so violent water does not lose geometric fidelity.
        let rough = build_promoted_clipmap(
            10.0,
            0.0,
            minimum,
            maximum,
            None,
            PROMOTED_CLIPMAP_FULL_DETAIL_AMPLITUDE,
        );
        assert_eq!(rough.vertices.len(), reference.vertices.len());
        assert_eq!(rough.indices.len(), reference.indices.len());
    }

    #[test]
    fn runtime_promoted_lod_uses_true_3d_distance_and_live_amplitude() {
        let minimum = [-20_000.0; 2];
        let maximum = [20_000.0; 2];
        let surface = OceanSurface { plane_height: 0.0, minimum, maximum };
        let runtime = build_runtime_promoted_clipmap(
            OCEAN_HIGH_MESH_SPACING_UNITS,
            0.0,
            minimum,
            maximum,
            None,
            appearance::MIN_PROMOTED_AMPLITUDE,
        );
        let calm_half = promoted_clipmap_half_cells(appearance::MIN_PROMOTED_AMPLITUDE);
        assert_eq!(calm_half, 32);
        assert_eq!(promoted_surface_distance([0.0, 0.0, 0.0], surface), 0.0);

        // Renderer Y is JKA Z: a camera one thousand units straight above the
        // water has crossed the first two calm-water LOD radii even though its
        // horizontal coordinates are unchanged.
        let sky_distance = promoted_surface_distance([0.0, 1000.0, 0.0], surface);
        assert_eq!(sky_distance, 1000.0);
        let sky_level = runtime.layout.minimum_level_for_distance(calm_half, sky_distance);
        assert_eq!(sky_level, 2);
        assert!(runtime.layout.indices_for(calm_half, sky_level).len() < runtime.indices.len());

        // The same camera distance stays in a finer level when amplitude grows,
        // proving the runtime slider changes the physical LOD thresholds.
        let rough_half = promoted_clipmap_half_cells(16.0);
        assert!(rough_half > calm_half);
        let test_distance = 500.0;
        assert!(runtime.layout.minimum_level_for_distance(calm_half, test_distance)
            > runtime.layout.minimum_level_for_distance(rough_half, test_distance));
        assert!(runtime.layout.indices_for(rough_half, 0).len() > runtime.indices.len());
        assert!(runtime.index_capacity >= runtime.layout.indices_for(rough_half, 0).len());

        // Horizontal separation uses the same shortest 3D distance metric.
        assert_eq!(promoted_surface_distance([30_000.0, 0.0, 0.0], surface), 10_000.0);
    }

    #[test]
    fn level_limit_drops_only_rings_beyond_the_radius() {
        let (minimum, maximum) = ([-20_000.0; 2], [20_000.0; 2]);
        let runtime = build_runtime_promoted_clipmap(
            OCEAN_HIGH_MESH_SPACING_UNITS, 0.0, minimum, maximum, None, appearance::MIN_PROMOTED_AMPLITUDE);
        let surface = OceanSurface { plane_height: 0.0, minimum, maximum };
        let masks = OceanMasks::default();
        let half = promoted_clipmap_half_cells(appearance::MIN_PROMOTED_AMPLITUDE);
        let layout = &runtime.layout;
        let all = layout.levels_within(half, 0.0, f32::INFINITY);
        assert!(all > 2);
        // A tiny radius needs only the finest level; each level end is a threshold.
        assert_eq!(layout.levels_within(half, 0.0, 1.0), 1);
        let reach0 = layout.base_spacing * promoted_level_half_cells(0, half) as f32;
        assert_eq!(layout.levels_within(half, 0.0, reach0 + 1.0), 2);
        // A viewer off the centre can see further rings.
        assert!(layout.levels_within(half, reach0, reach0 + 1.0) >= 2);
        // Capping removes the outer indices and keeps a strict prefix of the cells.
        let full = layout.indices_for_footprint_with_sky(half, 0, u32::MAX, [0.0; 2], surface, &masks, None, None, None);
        let capped = layout.indices_for_footprint_with_sky(half, 0, 2, [0.0; 2], surface, &masks, None, None, None);
        assert!(capped.len() < full.len());
        assert_eq!(&full[..capped.len()], &capped[..]);
        // The finest level is never dropped, even for a limit below it.
        let floor = layout.indices_for_footprint_with_sky(half, 0, 0, [0.0; 2], surface, &masks, None, None, None);
        assert!(!floor.is_empty());
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
            plane, minimum: [x[0], z[0]], maximum: [x[1], z[1]], authored_ocean: None, material: 0,
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
        other.material = 1;
        assert_eq!(cluster_water_faces(&[faces[1], other]).len(), 2);
        other.material = 0;
        other.authored_ocean = Some(0);
        assert_eq!(cluster_water_faces(&[faces[1], other]).len(), 2);
        assert_eq!(cluster_water_faces(&[faces[1], face(900.0, [0.0, 4000.0], [0.0, 4000.0])]).len(), 2);
    }

    #[test]
    fn rectangular_promoted_water_needs_no_fragment_mask() {
        let triangles: [MaskTriangle; 2] = [
            [[0.0,0.0],[100.0,0.0],[0.0,50.0]],
            [[100.0,0.0],[100.0,50.0],[0.0,50.0]],
        ];
        assert!(!promoted_outline_needs_mask([0.0,0.0],[100.0,50.0],&triangles));
    }

    #[test]
    fn irregular_promoted_water_keeps_fragment_mask() {
        // One quarter of the 100x100 footprint is deliberately missing.
        let triangles: [MaskTriangle; 4] = [
            [[0.0,0.0],[100.0,0.0],[0.0,50.0]],
            [[100.0,0.0],[100.0,50.0],[0.0,50.0]],
            [[0.0,50.0],[50.0,50.0],[0.0,100.0]],
            [[50.0,50.0],[50.0,100.0],[0.0,100.0]],
        ];
        assert!(promoted_outline_needs_mask([0.0,0.0],[100.0,100.0],&triangles));
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
        assert_eq!(masks.bounds[2], [0.0; 4]);
        assert_eq!(masks.tiles[2], [0; 4]);
        assert_eq!(masks.prune_tiles[2], [0; OCEAN_PRUNE_TILE_WORDS]);
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

#[cfg(test)]
mod ocean_phase_scheduler_tests {
    use super::*;

    #[test]
    fn spreads_fifteen_passes_over_one_tick() {
        assert_eq!(ocean_phases_due(0.0, 0.0, 1.0/24.0, 15), 1);
        assert_eq!(ocean_phases_due(1.0/48.0, 0.0, 1.0/24.0, 15), 8);
        assert_eq!(ocean_phases_due(1.0/24.0, 0.0, 1.0/24.0, 15), 15);
        assert_eq!(ocean_phases_due(0.005, 0.0, 1.0/24.0, 15), 2);
    }

    #[test]
    fn mesh_lod_deadlines_keep_32_and_24_hz_without_frame_quantization() {
        for fps in [60,120,240,1000] {
            let mut next = [0.0f32;2];
            let mut delivered = [0usize;2];
            for frame in 0..fps*5 {
                let now = frame as f32/fps as f32;
                for (i,hz) in [32.0,24.0].iter().enumerate() {
                    if ocean_lod_deadline(&mut next[i],now,*hz) { delivered[i]+=1; }
                }
            }
            for (i,hz) in [32.0,24.0].iter().enumerate() {
                assert!((delivered[i] as f32/5.0 - hz).abs() < 1.0,
                    "{fps} fps mesh region {i}: expected {hz}, delivered {:?}",delivered);
            }
        }
    }

    #[test]
    fn mesh_lod_toggle_is_not_a_fft_cascade_scheduler() {
        assert!(!OceanSettings::default().dynamic_update_rates);
        assert_eq!(OCEAN_FFT_PASSES,OCEAN_PASSES_PER_CASCADE * OCEAN_CASCADES);
    }

    #[test]
    fn handles_zero_rate_and_clamps_overdue_phases() {
        assert_eq!(ocean_phases_due(0.0, 0.0, 0.0, 15), 15);
        assert_eq!(ocean_phases_due(10.0, 0.0, 1.0/60.0, 16), 16);
        assert_eq!(ocean_phases_due(-1.0, 0.0, 1.0/60.0, 16), 1);
    }
}

#[cfg(test)]
mod ocean_fft_heading_tests {
    use super::*;
    use futures_lite::future::block_on;

    fn f16_to_f32(bits: u16) -> f32 {
        let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
        let exp = ((bits >> 10) & 0x1f) as i32;
        let frac = (bits & 0x3ff) as f32;
        if exp == 0 { sign * frac * 2f32.powi(-24) } else { sign * (1.0 + frac / 1024.0) * 2f32.powi(exp - 15) }
    }

    fn angle_difference(a: f32, b: f32) -> f32 {
        (a - b + 540.0).rem_euclid(360.0) - 180.0
    }

    /// Measures which way the FFT crests (and so the Jacobian whitecap foam
    /// laid down on them) really travel, in renderer XZ, and requires that to
    /// follow the swell `bsp.wgsl` animates -- the dominant promoted pool's
    /// heading when there is one, not the global `r_oceanAuthoring` heading.
    #[test]
    #[ignore = "requires a Vulkan GPU"]
    fn fft_travel_follows_dominant_pool_swell_heading() {
        use crate::ocean::appearance::{wave_heuristic, PromotedOcean};
        block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance.request_adapter(&Default::default()).await.unwrap();
            let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(), ..Default::default()
            }).await.unwrap();
            let n = 256u32;
            let albedo = device.create_texture(&wgpu::TextureDescriptor {
                label: None, size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let albedo_view = albedo.create_view(&Default::default());
            let layout = OceanGpu::create_render_layout(&device);
            // (global cvar heading, promoted pool swell heading)
            for (cvar, pool_swell) in [(0.0f32, None), (90.0, None), (180.0, None), (0.0, Some(180.0f32)), (0.0, Some(90.0))] {
                let expected = pool_swell.unwrap_or(cvar);
                let mut settings = OceanSettings { map_size: n, updates_per_second: 0.0, ..Default::default() };
                settings.authored.direction = cvar;
                settings.wind.speed = 0.0;
                let mut ocean = OceanGpu::new(&device, &queue, &layout, &albedo_view, settings);
                ocean.set_enabled(&queue, true);
                if let Some(swell) = pool_swell {
                    let pool = PromotedOcean { index: 0, name: "pool".into(),
                        surface: OceanSurface { plane_height: 100.0, minimum: [0.0; 2], maximum: [4096.0; 2] },
                        heuristic: wave_heuristic(0.0, None), color: [0.1, 0.2, 0.3], roughness: None,
                        source_deformation: None, source_alpha: None,
                        swell_direction: Some(swell) };
                    ocean.set_surfaces(&queue, &[pool.surface]);
                    ocean.set_promoted_surfaces(&device, &queue, &[pool], &[]);
                }
                let read = |ocean: &mut OceanGpu, layer: u32| -> Vec<f32> {
                    let mut encoder = device.create_command_encoder(&Default::default());
                    ocean.encode_frame(&queue, &mut encoder, 0.3);
                    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: None, size: u64::from(n * n * 8),
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    });
                    encoder.copy_texture_to_buffer(
                        wgpu::TexelCopyTextureInfo { texture: &ocean._displacement, mip_level: 0,
                            origin: wgpu::Origin3d { x: 0, y: 0, z: layer }, aspect: wgpu::TextureAspect::All },
                        wgpu::TexelCopyBufferInfo { buffer: &buffer, layout: wgpu::TexelCopyBufferLayout {
                            offset: 0, bytes_per_row: Some(n * 8), rows_per_image: Some(n) } },
                        wgpu::Extent3d { width: n, height: n, depth_or_array_layers: 1 });
                    let submission = queue.submit([encoder.finish()]);
                    let (tx, rx) = std::sync::mpsc::channel();
                    buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
                    device.poll(wgpu::PollType::Wait { submission_index: Some(submission),
                        timeout: Some(std::time::Duration::from_secs(60)) }).unwrap();
                    rx.recv().unwrap().unwrap();
                    let data = buffer.slice(..).get_mapped_range().expect("range");
                    // Texel .y is the vertical displacement.
                    (0..(n * n) as usize).map(|i| f16_to_f32(u16::from_le_bytes([data[i * 8 + 2], data[i * 8 + 3]]))).collect()
                };
                // Settle: the first frames also regenerate the spectrum.
                for _ in 0..6 { read(&mut ocean, 0); }
                // Cascade 1 carries a +15 degree offset from the heading.
                let layer = 1;
                let before = read(&mut ocean, layer);
                let after = read(&mut ocean, layer);
                let (mut best, mut shift) = (f64::MAX, (0i32, 0i32));
                for sy in -16i32..=16 { for sx in -16i32..=16 {
                    let mut sse = 0.0f64;
                    for y in (0..n as i32).step_by(4) { for x in (0..n as i32).step_by(4) {
                        let px = (x - sx).rem_euclid(n as i32) as usize;
                        let py = (y - sy).rem_euclid(n as i32) as usize;
                        let d = f64::from(after[y as usize * n as usize + x as usize] - before[py * n as usize + px]);
                        sse += d * d;
                    }}
                    if sse < best { best = sse; shift = (sx, sy); }
                }}
                // FFT grid is (x, -z): world travel = (sx, -sy); heading a = (cos a, -sin a).
                let travel = (-(-shift.1 as f32)).atan2(shift.0 as f32).to_degrees();
                let error = angle_difference(travel, expected + 15.0);
                println!("cvar {cvar:>5} pool {pool_swell:?}: grid shift {shift:?} -> travel {travel:>7.1} deg, expected {:>6.1} (error {error:>6.1})", expected + 15.0);
                assert!(error.abs() < 35.0, "FFT crests travel {travel} deg, swell heading is {expected}");
            }
        });
    }
}

#[cfg(test)]
mod ocean_backend_selection_tests {
    use super::{BlueWaterOptics, OceanSettings};

    #[test]
    fn hybrid_appearance_controls_both_media_without_mutating_native_settings() {
        let mut settings = OceanSettings::default();
        settings.backend = 2;
        settings.hybrid_transparency = 2.0;
        settings.hybrid_fog_color = [0.8, 0.3, 0.1];
        let saved = settings;
        let resolved = settings.resolved_for_gpu().sanitize();
        assert_eq!(resolved.optics.fog_color, [0.8, 0.3, 0.1]);
        assert_eq!(resolved.blue_water_optics.fog_color, [0.8, 0.3, 0.1]);
        assert_eq!(resolved.optics.transparency, super::optics::Settings::default().transparency * 2.0);
        // A murky saved Godot preset must not leak into Hybrid's submerged optics.
        let mut murky = saved;
        murky.optics.transparency = 6.0;
        assert_eq!(murky.resolved_for_gpu().optics.transparency, resolved.optics.transparency);
        assert_eq!(resolved.blue_water_optics.transparency, 2.0);
        assert_eq!(settings.blue_water_optics, BlueWaterOptics::default());
        settings.backend = 1;
        assert_eq!(settings.resolved_for_gpu().blue_water_optics, BlueWaterOptics::default());
    }

    #[test]
    fn hybrid_caustic_modes_are_exclusive() {
        let mut hybrid = OceanSettings::default();
        hybrid.backend = 2;
        hybrid.optics.caustics = 3.0;
        for mode in [0, 1, 2] {
            hybrid.hybrid_caustics_mode = mode;
            let resolved = hybrid.resolved_for_gpu();
            assert_eq!(resolved.godot_capture_caustics_strength(), 0.0);
            assert_eq!(resolved.blue_water_caustics_mode, mode);
            // Do not zero the master value: native modes still need it.
            assert_eq!(resolved.optics.caustics, 3.0);
        }
        for mode in [3, 4] {
            hybrid.hybrid_caustics_mode = mode;
            let resolved = hybrid.resolved_for_gpu();
            assert_eq!(resolved.blue_water_caustics_mode, mode);
            assert_eq!(resolved.godot_capture_caustics_strength(), 3.0);
            assert_eq!(resolved.sanitize().blue_water_caustics_mode, mode);
        }
        let mut native = OceanSettings::default();
        native.backend = 1;
        native.optics.caustics = 3.0;
        assert_eq!(native.resolved_for_gpu().godot_capture_caustics_strength(), 0.0);
        let mut godot = OceanSettings::default();
        godot.backend = 0;
        assert_eq!(godot.godot_capture_caustics_strength(), godot.optics.caustics);
    }

    #[test]
    fn native_and_hybrid_optics_are_independent() {
        let mut native = OceanSettings::default();
        native.backend = 1;
        assert_eq!(native.resolved_for_gpu().blue_water_refraction_mode, 2);
        assert_eq!(native.resolved_for_gpu().blue_water_caustics_mode, 0);
        assert_eq!(native.resolved_for_gpu().blue_water_underwater_mode, 0);
        assert!(native.resolved_for_gpu().blue_water_ssr);

        let mut hybrid = native;
        hybrid.backend = 2;
        let resolved = hybrid.resolved_for_gpu().sanitize();
        assert_eq!(resolved.blue_water_refraction_mode, 1);
        assert_eq!(resolved.blue_water_caustics_mode, 3);
        assert_eq!(resolved.blue_water_underwater_mode, 4);
        assert!(!resolved.blue_water_ssr);
        // Old configs that contained Hybrid's former Bluewater controls must
        // not make Native silently invoke the Godot receiver/submerged path.
        native.blue_water_caustics_mode = 3;
        native.blue_water_underwater_mode = 4;
        assert_eq!(native.resolved_for_gpu().blue_water_caustics_mode, 0);
        assert_eq!(native.resolved_for_gpu().blue_water_underwater_mode, 0);
        // Resolving one backend must never overwrite the persisted selection
        // for the other backend.
        assert_eq!(hybrid.blue_water_refraction_mode, 2);
        assert_eq!(hybrid.blue_water_caustics_mode, 0);
    }
}

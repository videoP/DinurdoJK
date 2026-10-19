//! Main-view face-plane culling without rewriting packed PVS/drawsurf geometry.
//! Homogeneous batches can be rejected wholesale. CPU world rendering may
//! instead draw surviving original BSP-surface index runs within mixed batches.
//! GPU-driven rendering retains the homogeneous-batch fallback.
use crate::ocean::optics::WaterCull;
use crate::renderer::{scene, CullMode, DrawBatch, DrawClass, FacePlaneRun, GpuVertex, LegacyDlightRun, WorldBatch};

const PLANE_EPSILON: f32 = 0.001;
const DISTANCE_EPSILON: f32 = 0.125;
/// Same plane-side tolerance as TaystJK/OpenJK R_CullSurface (in JKA units).
const FACE_EPSILON: f32 = 8.0;
const NO_PLANE: [f32; 4] = [0.0; 4];

/// A zero normal means "mixed/unknown: never face-plane-cull".
/// For front-sided surfaces the normal points to the renderable side. For
/// back-sided surfaces it is reversed, letting both use one signed-distance test.
pub(in crate::renderer) fn packed_face_plane(
    source: &DrawBatch,
    runs: &[LegacyDlightRun],
    surfaces: &[scene::LegacyDlightSurface],
    vertices: &[GpuVertex],
) -> [f32; 4] {
    if source.pipeline.cull == CullMode::None || source.pipeline.offset || source.water_deformed
        || source.pipeline.class == crate::renderer::DrawClass::Sky
        || source.water_primary || runs.is_empty()
    {
        return NO_PLANE;
    }
    let mut plane = None;
    for run in runs {
        let Some(surface) = surfaces.get(run.surface_id as usize) else {
            return NO_PLANE;
        };
        // A zero plane is a non-planar or unknown BSP surface. SURF_NODLIGHT
        // still has a valid plane, but remains excluded from legacy lighting.
        if surface.plane[..3] == [0.0; 3] {
            return NO_PLANE;
        }
        let next = surface.plane;
        if !next.iter().all(|v| v.is_finite()) {
            return NO_PLANE;
        }
        if let Some(previous) = plane {
            if !same_plane(previous, next) {
                return NO_PLANE;
            }
        } else {
            plane = Some(next);
        }
    }
    let Some(mut plane) = plane else { return NO_PLANE; };
    // The source BSP surface may have undergone geometry rewriting (e.g.
    // autosprite2). Require actual uploaded vertices to remain on this plane;
    // such animated/nonplanar replacements must never inherit the BSP plane.
    let start = source.vertices.start as usize;
    let end = source.vertices.end as usize;
    let Some(positions) = vertices.get(start..end) else { return NO_PLANE; };
    if positions.is_empty() || positions.iter().any(|v| {
        let p = v.position;
        let deviation = plane[0] * p[0] + plane[1] * p[1]
            + plane[2] * p[2] - plane[3];
        !deviation.is_finite() || deviation.abs() > DISTANCE_EPSILON
    }) {
        return NO_PLANE;
    }
    // Source material cull is expressed in JKA geometry-facing semantics.
    if source.pipeline.cull == CullMode::Front {
        for value in &mut plane { *value = -*value; }
    }
    plane
}

fn same_plane(a: [f32; 4], b: [f32; 4]) -> bool {
    (0..3).all(|i| (a[i] - b[i]).abs() <= PLANE_EPSILON)
        && (a[3] - b[3]).abs() <= DISTANCE_EPSILON
}

/// Validate a single original BSP-surface run before using its plane. A
/// collocated but non-planar vertex must not inherit a face plane, and authored
/// modifiers (offset/water/deformation) are excluded entirely.
pub(in crate::renderer) fn authored_face_runs(
    source: &DrawBatch,
    batch_indices: &std::ops::Range<u32>,
    runs: &[LegacyDlightRun],
    surfaces: &[scene::LegacyDlightSurface],
    vertices: &[GpuVertex],
) -> Vec<FacePlaneRun> {
    // Runs also carry per-surface bounds for the water cull, so cull-none and
    // transparent surfaces get them too (with no plane, they are never face-culled).
    if runs.is_empty() || source.pipeline.offset || source.water_deformed || source.water_primary
        || source.pipeline.class == DrawClass::Sky
    { return Vec::new(); }
    let planar = source.pipeline.cull != CullMode::None && source.pipeline.class != DrawClass::Transparent;
    let mut result = Vec::with_capacity(runs.len());
    let mut cursor = 0u32;
    let count = batch_indices.end.saturating_sub(batch_indices.start);
    for run in runs {
        let Some(begin) = run.indexed_range.start.checked_sub(batch_indices.start) else { return Vec::new(); };
        let Some(end) = run.indexed_range.end.checked_sub(batch_indices.start) else { return Vec::new(); };
        if begin != cursor || end > count || begin >= end { return Vec::new(); }
        cursor = end;
        let surface = surfaces.get(run.surface_id as usize);
        let (bounds_min, bounds_max) = surface
            .filter(|s| s.bounds_min.iter().chain(&s.bounds_max).all(|v| v.is_finite()) && s.bounds_min != s.bounds_max)
            .map_or(FacePlaneRun::UNKNOWN_BOUNDS, |s| (s.bounds_min, s.bounds_max));
        let mut plane = surface.map_or(NO_PLANE, |s| s.plane);
        if !planar || plane[..3] == [0.0; 3] || !plane.iter().all(|p| p.is_finite()) {
            plane = NO_PLANE;
        } else {
            let first = source.vertices.start as usize + begin as usize;
            let last = source.vertices.start as usize + end as usize;
            match vertices.get(first..last) {
                Some(positions) if positions.iter().all(|v| {
                    let p = v.position;
                    let delta = plane[0]*p[0] + plane[1]*p[1] + plane[2]*p[2] - plane[3];
                    delta.is_finite() && delta.abs() <= DISTANCE_EPSILON
                }) => {
                    if source.pipeline.cull == CullMode::Front {
                        for value in &mut plane { *value = -*value; }
                    }
                }
                _ => plane = NO_PLANE,
            }
        }
        result.push(FacePlaneRun { offsets: begin..end, plane, bounds_min, bounds_max });
    }
    if cursor != count { return Vec::new(); }
    result
}

/// Construct relocatable runs for an AUTO-4 recipe. The order is *exactly* the
/// FULL geometry-member concatenation order used by the collapse worker.
pub(in crate::renderer) fn merged_face_runs<'a>(
    members: impl Iterator<Item = &'a WorldBatch>,
) -> Vec<FacePlaneRun> {
    let mut result = Vec::new();
    let mut offset = 0u32;
    for member in members {
        let count = member.indexed_range.end.saturating_sub(member.indexed_range.start);
        if count == 0 { continue; }
        if member.face_cull_runs.is_empty() {
            result.push(FacePlaneRun {
                offsets: offset..offset.saturating_add(count),
                plane: NO_PLANE,
                bounds_min: member.bounds_min,
                bounds_max: member.bounds_max,
            });
        } else {
            for run in member.face_cull_runs.iter() {
                result.push(FacePlaneRun {
                    offsets: offset.saturating_add(run.offsets.start)..offset.saturating_add(run.offsets.end),
                    plane: run.plane,
                    bounds_min: run.bounds_min,
                    bounds_max: run.bounds_max,
                });
            }
        }
        offset = offset.saturating_add(count);
    }
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::renderer) enum FacePlaneSelection {
    Keep,
    Reject,
    Partial,
}

/// The index range the face runs describe. Usually the live range; a batch whose
/// live range is rewritten per frame (patch LOD) keeps the immutable FULL layout
/// the runs were built against in `full_indexed_range`.
fn run_range(batch: &WorldBatch) -> Option<std::ops::Range<u32>> {
    let total = batch.face_cull_runs.last()?.offsets.end;
    [&batch.indexed_range, &batch.full_indexed_range]
        .into_iter()
        .find(|range| range.end.saturating_sub(range.start) == total)
        .cloned()
}

/// Everything that can remove part of a world batch on the CPU path.
pub(in crate::renderer) struct RunCull<'a> {
    pub camera: [f32; 3],
    /// Reject runs whose BSP face plane faces away (r_facePlaneCull).
    pub face_planes: bool,
    /// Reject runs that water absorption hides.
    pub water: Option<&'a WaterCull<'a>>,
}

impl RunCull<'_> {
    fn run_hidden_by_water(&self, run: &FacePlaneRun) -> bool {
        self.water.is_some_and(|water| water.culls(run.bounds_min, run.bounds_max))
    }
    fn rejects(&self, run: &FacePlaneRun) -> bool {
        (self.face_planes && face_plane_rejected(run.plane, self.camera)) || self.run_hidden_by_water(run)
    }
}

/// An entire batch may contain front- and back-facing *original BSP surfaces*.
/// This test does not split PVS batches: only their GPU index submission.
pub(in crate::renderer) fn face_plane_selection(batch: &WorldBatch, cull: &RunCull) -> FacePlaneSelection {
    selection_stats(batch, cull).0
}

/// Like [`face_plane_selection`], also returning how many indices the water
/// cull alone removed (a whole batch counts all of its indices).
pub(in crate::renderer) fn selection_stats(batch: &WorldBatch, cull: &RunCull) -> (FacePlaneSelection, u32) {
    let count = batch.indexed_range.end.saturating_sub(batch.indexed_range.start);
    if batch.indexed_range.is_empty() {
        return (FacePlaneSelection::Keep, 0);
    }
    // Inline models (func_static etc.) carry live world-space bounds, re-posed every frame.
    if batch.source.pipeline.class != DrawClass::Sky
        && cull.water.is_some_and(|water| water.culls(batch.bounds_min, batch.bounds_max))
    {
        return (FacePlaneSelection::Reject, count);
    }
    if batch.is_inline_entity {
        return (FacePlaneSelection::Keep, 0);
    }
    if batch.patch_geometry.is_some() || batch.ocean_clipmap.is_some() || batch.face_cull_runs.is_empty() {
        let rejected = cull.face_planes && batch_face_plane_rejected(batch, cull.camera);
        return (if rejected { FacePlaneSelection::Reject } else { FacePlaneSelection::Keep }, 0);
    }
    let Some(range) = run_range(batch) else { return (FacePlaneSelection::Keep, 0); };
    let count = range.end - range.start;
    if cull.water.is_none() {
        // Face planes alone never split a batch: the rasterizer already drops
        // back faces, so a partial cull would only trade them for extra draws
        // and break the batch out of its compact multi-draw. Like JKA's
        // R_CullSurface, only a batch whose every surface faces away is skipped,
        // and the first surviving run ends the scan.
        if !cull.face_planes {
            return (FacePlaneSelection::Keep, 0);
        }
        let mut cursor = 0u32;
        for run in batch.face_cull_runs.iter() {
            if run.offsets.start != cursor || run.offsets.end > count || run.offsets.start >= run.offsets.end
                || !face_plane_rejected(run.plane, cull.camera)
            {
                return (FacePlaneSelection::Keep, 0);
            }
            cursor = run.offsets.end;
        }
        let selection = if cursor == count { FacePlaneSelection::Reject } else { FacePlaneSelection::Keep };
        return (selection, 0);
    }
    let mut removed = 0u32;
    let mut by_water = 0u32;
    let mut cursor = 0u32;
    for run in batch.face_cull_runs.iter() {
        if run.offsets.start != cursor || run.offsets.end > count || run.offsets.start >= run.offsets.end {
            return (FacePlaneSelection::Keep, 0);
        }
        cursor = run.offsets.end;
        let len = run.offsets.end - run.offsets.start;
        if cull.run_hidden_by_water(run) {
            by_water = by_water.saturating_add(len);
            removed = removed.saturating_add(len);
        } else if cull.face_planes && face_plane_rejected(run.plane, cull.camera) {
            removed = removed.saturating_add(len);
        }
    }
    if cursor != count { return (FacePlaneSelection::Keep, 0); }
    if removed == count { (FacePlaneSelection::Reject, by_water) }
    // Only water absorption justifies split draws; see the face-plane-only path.
    else if by_water > 0 { (FacePlaneSelection::Partial, by_water) }
    else { (FacePlaneSelection::Keep, 0) }
}

/// Main-camera CPU draw for partial batches. Adjacent surviving BSP face-runs
/// coalesce into one indexed draw. No new buffer, PVS plan or pipeline required.
/// Returns (draw calls, culled triangles).
pub(in crate::renderer) fn draw_partial_face_batch<'a>(
    pass: &mut wgpu::RenderPass<'a>, batch: &WorldBatch, cull: &RunCull,
) -> (u32, u32) {
    let mut pending: Option<std::ops::Range<u32>> = None;
    let mut draws = 0u32;
    let mut culled_indices = 0u32;
    let base = run_range(batch).map_or(batch.indexed_range.start, |range| range.start);
    for run in batch.face_cull_runs.iter() {
        if cull.rejects(run) {
            culled_indices += run.offsets.end - run.offsets.start;
            if let Some(range) = pending.take() {
                pass.draw_indexed(range, 0, 0..1);
                draws += 1;
            }
            continue;
        }
        let range = base.saturating_add(run.offsets.start)..base.saturating_add(run.offsets.end);
        if let Some(previous) = pending.as_mut() {
            if previous.end == range.start { previous.end = range.end; continue; }
        }
        if let Some(range) = pending.replace(range) {
            pass.draw_indexed(range, 0, 0..1);
            draws += 1;
        }
    }
    if let Some(range) = pending { pass.draw_indexed(range, 0, 0..1); draws += 1; }
    (draws, culled_indices / 3)
}

/// Only use for the current render camera. Reflection and shadow cameras must
/// evaluate their own origins, never inherit the main camera's result.
pub(in crate::renderer) fn face_plane_rejected(
    plane: [f32; 4], camera_position: [f32; 3],
) -> bool {
    if plane[0] == 0.0 && plane[1] == 0.0 && plane[2] == 0.0 {
        return false;
    }
    plane[0] * camera_position[0]
        + plane[1] * camera_position[1]
        + plane[2] * camera_position[2]
        < plane[3] - FACE_EPSILON
}

pub(in crate::renderer) fn batch_face_plane_rejected(
    batch: &WorldBatch, camera_position: [f32; 3],
) -> bool {
    !batch.is_inline_entity && face_plane_rejected(batch.face_cull_plane, camera_position)
}

/// A merged AUTO4 recipe may be culled only if *all* its constituent FULL
/// pieces share the exact same eligible face plane. Otherwise leave it intact.
pub(in crate::renderer) fn merged_face_plane<'a>(
    planes: impl Iterator<Item = &'a [f32; 4]>,
) -> [f32; 4] {
    let mut output = None;
    for &plane in planes {
        if plane[..3] == [0.0; 3] {
            return NO_PLANE;
        }
        if let Some(previous) = output {
            if !same_plane(previous, plane) {
                return NO_PLANE;
            }
        } else {
            output = Some(plane);
        }
    }
    output.unwrap_or(NO_PLANE)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tayst_eight_unit_margin() {
        let plane = [0.0, 1.0, 0.0, 50.0];
        assert!(face_plane_rejected(plane, [0.0, 41.0, 0.0]));
        assert!(!face_plane_rejected(plane, [0.0, 42.0, 0.0]));
        assert!(!face_plane_rejected(plane, [0.0, 50.0, 0.0]));
    }
    #[test]
    fn single_face_plane_controls_backside_only() {
        let up = [0.0, 1.0, 0.0, 10.0];
        assert!(face_plane_rejected(up, [0.0, 0.0, 0.0]));
        assert!(!face_plane_rejected(up, [0.0, 30.0, 0.0]));
    }
    #[test]
    fn merged_runs_respect_offset_and_non_cullable_geometry() {
        let up = [0.0, 1.0, 0.0, 10.0];
        let other = [0.0, 0.0, 0.0, 0.0];
        let runs = [
            FacePlaneRun { offsets: 0..6, plane: up, bounds_min: [0.0; 3], bounds_max: [1.0; 3] },
            FacePlaneRun { offsets: 6..12, plane: other, bounds_min: [0.0; 3], bounds_max: [1.0; 3] },
        ];
        let culled = runs.iter().filter(|r| face_plane_rejected(r.plane, [0.0; 3]))
            .map(|r| r.offsets.end - r.offsets.start).sum::<u32>();
        assert_eq!(culled, 6);
        assert!(!face_plane_rejected(other, [0.0; 3]));
    }
    #[test]
    fn mixed_and_unknown_planes_remain_visible() {
        let a = [0.0, 1.0, 0.0, 50.0];
        let b = [1.0, 0.0, 0.0, 50.0];
        let unknown = NO_PLANE;
        assert_eq!(merged_face_plane([&a, &b].into_iter()), NO_PLANE);
        assert_eq!(merged_face_plane([&a, &unknown].into_iter()), NO_PLANE);
        assert_eq!(merged_face_plane([&a, &a].into_iter()), a);
        assert!(!face_plane_rejected(NO_PLANE, [0.0; 3]));
    }
}

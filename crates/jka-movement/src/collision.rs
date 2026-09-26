use super::*;
use jka_assets::bsp::{Bsp, SurfaceKind};
use std::collections::HashMap;

const SOURCE_CELL_SIZE: f32 = 1024.0;
const SOURCE_MAX_BRUSH_CELLS: usize = 4096;
const SOURCE_MAX_QUERY_CELLS: usize = 32768;
const SOURCE_CLIP_EPSILON: f32 = 0.125;
const SOURCE_INSIDE_EPSILON: f32 = 0.01;
const SOURCE_CONTENTS_BODY: i32 = 0x0000_0100;

type SourceCell = (i32, i32, i32);

struct NativeWorld(NonNull<c_void>);
// CM globals and per-world checkcounts are serialized by the native recursive mutex.
unsafe impl Send for NativeWorld {}
unsafe impl Sync for NativeWorld {}
impl Drop for NativeWorld {
    fn drop(&mut self) {
        unsafe { ffi::jka_world_free(self.0.as_ptr()) };
    }
}

#[derive(Debug, Clone)]
pub struct SourceCollisionPlane {
    /// JKA/map-space plane normal. Brush interior is `dot(normal, point) <= distance`.
    pub normal: [f32; 3],
    pub distance: f32,
    pub surface_flags: i32,
}

#[derive(Debug, Clone)]
pub struct SourceCollisionBrush {
    pub planes: Vec<SourceCollisionPlane>,
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub contents: i32,
}

struct SourceWorld {
    brushes: Vec<SourceCollisionBrush>,
    cells: HashMap<SourceCell, Vec<usize>>,
    large_brushes: Vec<usize>,
}

enum CollisionBackend {
    Native(NativeWorld),
    Source(SourceWorld),
}

pub struct CollisionWorld {
    backend: Arc<CollisionBackend>,
    // Per-clone scratch: CollisionWorld is cloned into the local-server/prediction
    // owners, so source broadphase queries can deduplicate cell occupants without
    // a mutex or a HashSet allocation on every Pmove trace.
    source_visit: Vec<u32>,
    source_generation: u32,
    source_candidates_scratch: Vec<usize>,
}

impl Clone for CollisionWorld {
    fn clone(&self) -> Self {
        Self {
            backend: Arc::clone(&self.backend),
            source_visit: vec![0; self.source_visit.len()],
            source_generation: 1,
            source_candidates_scratch: Vec::new(),
        }
    }
}

fn source_cell(value: f32) -> i32 {
    (value / SOURCE_CELL_SIZE).floor() as i32
}

fn source_cell_bounds(mins: [f32; 3], maxs: [f32; 3]) -> ([i32; 3], [i32; 3]) {
    (
        [source_cell(mins[0]), source_cell(mins[1]), source_cell(mins[2])],
        [source_cell(maxs[0]), source_cell(maxs[1]), source_cell(maxs[2])],
    )
}

fn source_cell_count(minimum: [i32; 3], maximum: [i32; 3]) -> usize {
    let mut count = 1usize;
    for axis in 0..3 {
        let span = i64::from(maximum[axis]) - i64::from(minimum[axis]) + 1;
        if span <= 0 {
            return 0;
        }
        count = count.saturating_mul(span as usize);
    }
    count
}

fn aabb_overlaps(a_min: [f32; 3], a_max: [f32; 3], b_min: [f32; 3], b_max: [f32; 3]) -> bool {
    (0..3).all(|axis| a_min[axis] <= b_max[axis] && a_max[axis] >= b_min[axis])
}

impl CollisionWorld {
    /// Validates every native collision reference before passing the original RBSP to OpenJK.
    pub fn from_bsp(data: &[u8]) -> Result<Self, String> {
        let bsp = Bsp::parse(data).map_err(|e| e.to_string())?;
        if bsp.collision.is_none() || bsp.planes.is_empty() || bsp.shaders.is_empty() {
            return Err("BSP has no compiled collision tree".into());
        }
        if bsp.models.len() > 1024 {
            return Err("BSP exceeds OpenJK inline model limit".into());
        }
        for brush in &bsp.brushes {
            if brush.sides.len() < 6 {
                return Err("Collision brush lacks six axial bounds".into());
            }
            for side in 0..6 {
                let plane = bsp.planes[bsp.brush_sides[brush.sides.start + side].plane];
                let mut expected = [0.0; 3];
                expected[side / 2] = if side % 2 == 0 { -1.0 } else { 1.0 };
                if plane.normal != expected {
                    return Err("Collision brush axial planes are out of order".into());
                }
            }
        }
        for surface in &bsp.surfaces {
            if surface.kind == SurfaceKind::Patch
                && (surface.vertices.len() > 1024 || surface.patch_size.iter().any(|&n| n > 129))
            {
                return Err("Patch exceeds OpenJK collision grid limit".into());
            }
        }
        let raw = unsafe { ffi::jka_world_new(data.as_ptr(), data.len() as i32) };
        let native = NonNull::new(raw).ok_or_else(error)?;
        Ok(Self {
            backend: Arc::new(CollisionBackend::Native(NativeWorld(native))),
            source_visit: Vec::new(),
            source_generation: 1,
            source_candidates_scratch: Vec::new(),
        })
    }

    /// Builds gameplay collision directly from convex source-map brushes. This
    /// deliberately does not replace the native OpenJK BSP collision path; it is
    /// only the fallback that lets a developer Join Game before q3map2 exists.
    pub fn from_source_brushes(brushes: Vec<SourceCollisionBrush>) -> Result<Self, String> {
        if brushes.is_empty() {
            return Err("Source map has no gameplay collision brushes".into());
        }
        let mut cells = HashMap::<SourceCell, Vec<usize>>::new();
        let mut large_brushes = Vec::new();
        let mut valid_brushes = 0usize;
        for (index, brush) in brushes.iter().enumerate() {
            if brush.planes.len() < 4
                || brush.contents == 0
                || brush.mins.iter().chain(&brush.maxs).any(|value| !value.is_finite())
                || brush.planes.iter().any(|plane| {
                    !plane.distance.is_finite() || plane.normal.iter().any(|value| !value.is_finite())
                })
            {
                continue;
            }
            valid_brushes += 1;
            let (minimum, maximum) = source_cell_bounds(brush.mins, brush.maxs);
            let count = source_cell_count(minimum, maximum);
            if count == 0 || count > SOURCE_MAX_BRUSH_CELLS {
                large_brushes.push(index);
                continue;
            }
            for z in minimum[2]..=maximum[2] {
                for y in minimum[1]..=maximum[1] {
                    for x in minimum[0]..=maximum[0] {
                        cells.entry((x, y, z)).or_default().push(index);
                    }
                }
            }
        }
        if valid_brushes == 0 {
            return Err("Source map has no valid gameplay collision brushes".into());
        }
        let brush_count = brushes.len();
        Ok(Self {
            backend: Arc::new(CollisionBackend::Source(SourceWorld {
                brushes,
                cells,
                large_brushes,
            })),
            source_visit: vec![0; brush_count],
            source_generation: 1,
            source_candidates_scratch: Vec::new(),
        })
    }

    pub fn is_source_map(&self) -> bool {
        matches!(&*self.backend, CollisionBackend::Source(_))
    }

    pub fn source_brush_count(&self) -> usize {
        match &*self.backend {
            CollisionBackend::Source(world) => world.brushes.len(),
            CollisionBackend::Native(_) => 0,
        }
    }

    fn begin_source_query(&mut self) -> u32 {
        self.source_generation = self.source_generation.wrapping_add(1);
        if self.source_generation == 0 {
            self.source_visit.fill(0);
            self.source_generation = 1;
        }
        self.source_generation
    }

    fn source_candidates(&mut self, minimum: [f32; 3], maximum: [f32; 3]) -> Vec<usize> {
        let generation = self.begin_source_query();
        let backend = Arc::clone(&self.backend);
        let CollisionBackend::Source(world) = &*backend else {
            return Vec::new();
        };
        let (cell_min, cell_max) = source_cell_bounds(minimum, maximum);
        let query_cells = source_cell_count(cell_min, cell_max);
        let mut result = std::mem::take(&mut self.source_candidates_scratch);
        result.clear();
        let add = |index: usize, result: &mut Vec<usize>, visit: &mut [u32]| {
            if visit.get(index).copied() != Some(generation) {
                if let Some(slot) = visit.get_mut(index) {
                    *slot = generation;
                    result.push(index);
                }
            }
        };

        // A pathological very long trace is cheaper as one linear AABB sweep
        // than touching tens of thousands of empty hash-grid cells.
        if query_cells == 0 || query_cells > SOURCE_MAX_QUERY_CELLS {
            for (index, brush) in world.brushes.iter().enumerate() {
                if aabb_overlaps(minimum, maximum, brush.mins, brush.maxs) {
                    add(index, &mut result, &mut self.source_visit);
                }
            }
            return result;
        }

        for z in cell_min[2]..=cell_max[2] {
            for y in cell_min[1]..=cell_max[1] {
                for x in cell_min[0]..=cell_max[0] {
                    if let Some(indices) = world.cells.get(&(x, y, z)) {
                        for &index in indices {
                            add(index, &mut result, &mut self.source_visit);
                        }
                    }
                }
            }
        }
        for &index in &world.large_brushes {
            if let Some(brush) = world.brushes.get(index) {
                if aabb_overlaps(minimum, maximum, brush.mins, brush.maxs) {
                    add(index, &mut result, &mut self.source_visit);
                }
            }
        }
        result
    }
}

fn error() -> String {
    unsafe {
        CStr::from_ptr(ffi::jka_collision_error())
            .to_string_lossy()
            .into_owned()
    }
}

/// A solid snapshot entity as CG_ClipMoveToEntities sees it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EntityClip {
    /// SOLID_BMODEL: inline model `*index` placed at origin/angles.
    InlineModel { index: i32, origin: [f32; 3], angles: [f32; 3] },
    /// Encoded bbox (CM_TempBoxModel) at origin, never rotated.
    Box { mins: [f32; 3], maxs: [f32; 3], origin: [f32; 3] },
}

impl CollisionWorld {
    /// CM_TransformedBoxTrace against one entity. `entity` stays ENTITY_NONE;
    /// the caller assigns the entity number like CG_ClipMoveToEntities.
    pub fn trace_entity(&mut self, q: TraceQuery, clip: EntityClip) -> TraceResult {
        if self.is_source_map() {
            return match clip {
                EntityClip::InlineModel { .. } => {
                    // Uncompiled source maps do not yet instantiate moving brush
                    // models (`*1`, func_door, platforms, etc.).
                    TraceResult::clear(q.end)
                }
                EntityClip::Box { mins, maxs, origin } => {
                    // CM_TempBoxModel uses CONTENTS_BODY. Represent that exact
                    // axis-aligned world-space box as a six-plane convex brush.
                    let world_mins = [origin[0] + mins[0], origin[1] + mins[1], origin[2] + mins[2]];
                    let world_maxs = [origin[0] + maxs[0], origin[1] + maxs[1], origin[2] + maxs[2]];
                    let brush = SourceCollisionBrush {
                        planes: vec![
                            SourceCollisionPlane { normal: [1.0, 0.0, 0.0], distance: world_maxs[0], surface_flags: 0 },
                            SourceCollisionPlane { normal: [-1.0, 0.0, 0.0], distance: -world_mins[0], surface_flags: 0 },
                            SourceCollisionPlane { normal: [0.0, 1.0, 0.0], distance: world_maxs[1], surface_flags: 0 },
                            SourceCollisionPlane { normal: [0.0, -1.0, 0.0], distance: -world_mins[1], surface_flags: 0 },
                            SourceCollisionPlane { normal: [0.0, 0.0, 1.0], distance: world_maxs[2], surface_flags: 0 },
                            SourceCollisionPlane { normal: [0.0, 0.0, -1.0], distance: -world_mins[2], surface_flags: 0 },
                        ],
                        mins: world_mins,
                        maxs: world_maxs,
                        contents: SOURCE_CONTENTS_BODY,
                    };
                    let mut result = TraceResult::clear(q.end);
                    source_trace_brush(q, &brush, &mut result);
                    result.entity = ENTITY_NONE;
                    result
                }
            };
        }

        let CollisionBackend::Native(native) = &*self.backend else { unreachable!() };
        let mut result = TraceResult::clear(q.end);
        let ok = unsafe {
            match clip {
                EntityClip::InlineModel { index, origin, angles } => ffi::jka_world_trace(
                    native.0.as_ptr(),
                    &mut result,
                    q.start.as_ptr(),
                    q.mins.as_ptr(),
                    q.maxs.as_ptr(),
                    q.end.as_ptr(),
                    q.mask,
                    index,
                    origin.as_ptr(),
                    angles.as_ptr(),
                ),
                EntityClip::Box { mins, maxs, origin } => ffi::jka_world_trace_box_entity(
                    native.0.as_ptr(),
                    &mut result,
                    q.start.as_ptr(),
                    q.mins.as_ptr(),
                    q.maxs.as_ptr(),
                    q.end.as_ptr(),
                    q.mask,
                    mins.as_ptr(),
                    maxs.as_ptr(),
                    origin.as_ptr(),
                ),
            }
        };
        assert_ne!(ok, 0, "{}", error());
        result.entity = ENTITY_NONE;
        result
    }

    /// CM_TransformedPointContents for an inline model.
    pub fn inline_model_contents(&mut self, point: [f32; 3], index: i32, origin: [f32; 3], angles: [f32; 3]) -> i32 {
        let CollisionBackend::Native(native) = &*self.backend else {
            return 0;
        };
        let contents = unsafe {
            ffi::jka_world_contents(native.0.as_ptr(), point.as_ptr(), index, origin.as_ptr(), angles.as_ptr())
        };
        assert_ne!(contents, -1, "{}", error());
        contents
    }
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn box_plane_min_support(normal: [f32; 3], mins: [f32; 3], maxs: [f32; 3]) -> f32 {
    let mut support = 0.0;
    for axis in 0..3 {
        support += normal[axis] * if normal[axis] >= 0.0 { mins[axis] } else { maxs[axis] };
    }
    support
}

fn source_trace_brush(q: TraceQuery, brush: &SourceCollisionBrush, best: &mut TraceResult) {
    if brush.contents & q.mask == 0 {
        return;
    }

    let mut enter_fraction = -1.0f32;
    let mut leave_fraction = 1.0f32;
    let mut clip_plane = None::<&SourceCollisionPlane>;
    let mut starts_out = false;
    let mut ends_out = false;

    for plane in &brush.planes {
        // Minkowski expansion for a point trace of the player-box origin.
        // For brush interior n.x <= d, subtract the box's minimum support.
        let expanded_distance = plane.distance - box_plane_min_support(plane.normal, q.mins, q.maxs);
        let start_distance = dot3(plane.normal, q.start) - expanded_distance;
        let end_distance = dot3(plane.normal, q.end) - expanded_distance;

        if start_distance > 0.0 {
            starts_out = true;
        }
        if end_distance > 0.0 {
            ends_out = true;
        }
        // Match OpenJK CM_PlaneCollision. In particular the epsilon test on
        // the endpoint is important for grazing traces along brush faces; the
        // simpler d2 >= d1 test makes source-map movement noticeably stickier.
        if start_distance > 0.0
            && (end_distance >= SOURCE_CLIP_EPSILON || end_distance >= start_distance)
        {
            return;
        }
        if start_distance <= 0.0 && end_distance <= 0.0 {
            continue;
        }

        let delta = start_distance - end_distance;
        if start_distance > end_distance {
            // Entering the brush. This is the algebraic equivalent of
            // OpenJK's comparison-before-division form.
            let numerator = start_distance - SOURCE_CLIP_EPSILON;
            let fraction = if numerator < 0.0 { 0.0 } else { numerator / delta };
            if fraction > enter_fraction {
                enter_fraction = fraction;
                clip_plane = Some(plane);
            }
        } else {
            // Leaving the brush. Preserve OpenJK's special case rather than
            // clamping; the sign of delta is significant here.
            let numerator = start_distance + SOURCE_CLIP_EPSILON;
            let fraction = if numerator < delta { 1.0 } else { numerator / delta };
            if fraction < leave_fraction {
                leave_fraction = fraction;
            }
        }
    }

    if !starts_out {
        best.start_solid = 1;
        best.contents |= brush.contents;
        if !ends_out {
            best.all_solid = 1;
            best.fraction = 0.0;
            best.end = q.start;
            best.entity = ENTITY_WORLD;
        }
        return;
    }
    if enter_fraction < leave_fraction && enter_fraction >= 0.0 && enter_fraction < best.fraction {
        let plane = clip_plane.expect("source collision entering plane");
        best.fraction = enter_fraction;
        for axis in 0..3 {
            best.end[axis] = q.start[axis] + enter_fraction * (q.end[axis] - q.start[axis]);
        }
        best.normal = plane.normal;
        best.distance = plane.distance;
        best.surface_flags = plane.surface_flags;
        best.contents = brush.contents;
        best.entity = ENTITY_WORLD;
    }
}

impl TraceWorld for CollisionWorld {
    fn trace(&mut self, q: TraceQuery) -> TraceResult {
        assert!(q
            .start
            .iter()
            .chain(&q.end)
            .chain(&q.mins)
            .chain(&q.maxs)
            .all(|v| v.is_finite()));

        if self.is_source_map() {
            let mut query_min = [0.0; 3];
            let mut query_max = [0.0; 3];
            for axis in 0..3 {
                query_min[axis] = (q.start[axis] + q.mins[axis])
                    .min(q.end[axis] + q.mins[axis])
                    - SOURCE_CLIP_EPSILON;
                query_max[axis] = (q.start[axis] + q.maxs[axis])
                    .max(q.end[axis] + q.maxs[axis])
                    + SOURCE_CLIP_EPSILON;
            }
            let candidates = self.source_candidates(query_min, query_max);
            let CollisionBackend::Source(world) = &*self.backend else { unreachable!() };
            let mut result = TraceResult::clear(q.end);
            for &index in &candidates {
                if let Some(brush) = world.brushes.get(index) {
                    source_trace_brush(q, brush, &mut result);
                }
            }
            self.source_candidates_scratch = candidates;
            return result;
        }

        let CollisionBackend::Native(native) = &*self.backend else { unreachable!() };
        let mut result = TraceResult::clear(q.end);
        let zero = [0.0; 3];
        let ok = unsafe {
            ffi::jka_world_trace(
                native.0.as_ptr(),
                &mut result,
                q.start.as_ptr(),
                q.mins.as_ptr(),
                q.maxs.as_ptr(),
                q.end.as_ptr(),
                q.mask,
                0,
                zero.as_ptr(),
                zero.as_ptr(),
            )
        };
        assert_ne!(ok, 0, "{}", error());
        result
    }

    fn point_contents(&mut self, point: [f32; 3], _pass: i32) -> i32 {
        assert!(point.iter().all(|v| v.is_finite()));
        if self.is_source_map() {
            let candidates = self.source_candidates(point, point);
            let CollisionBackend::Source(world) = &*self.backend else { unreachable!() };
            let mut contents = 0;
            'brush: for &index in &candidates {
                let Some(brush) = world.brushes.get(index) else { continue; };
                if !aabb_overlaps(point, point, brush.mins, brush.maxs) {
                    continue;
                }
                for plane in &brush.planes {
                    if dot3(plane.normal, point) - plane.distance > SOURCE_INSIDE_EPSILON {
                        continue 'brush;
                    }
                }
                contents |= brush.contents;
            }
            self.source_candidates_scratch = candidates;
            return contents;
        }

        let CollisionBackend::Native(native) = &*self.backend else { unreachable!() };
        let zero = [0.0; 3];
        let contents = unsafe {
            ffi::jka_world_contents(
                native.0.as_ptr(),
                point.as_ptr(),
                0,
                zero.as_ptr(),
                zero.as_ptr(),
            )
        };
        assert_ne!(contents, -1, "{}", error());
        contents
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> SourceCollisionBrush {
        SourceCollisionBrush {
            planes: vec![
                SourceCollisionPlane { normal: [1.0, 0.0, 0.0], distance: 64.0, surface_flags: 0 },
                SourceCollisionPlane { normal: [-1.0, 0.0, 0.0], distance: 64.0, surface_flags: 0 },
                SourceCollisionPlane { normal: [0.0, 1.0, 0.0], distance: 64.0, surface_flags: 0 },
                SourceCollisionPlane { normal: [0.0, -1.0, 0.0], distance: 64.0, surface_flags: 0 },
                SourceCollisionPlane { normal: [0.0, 0.0, 1.0], distance: 64.0, surface_flags: 0 },
                SourceCollisionPlane { normal: [0.0, 0.0, -1.0], distance: 64.0, surface_flags: 0 },
            ],
            mins: [-64.0; 3],
            maxs: [64.0; 3],
            contents: 1,
        }
    }

    #[test]
    fn source_point_contents_cube() {
        let mut world = CollisionWorld::from_source_brushes(vec![cube()]).unwrap();
        assert_eq!(world.point_contents([0.0, 0.0, 0.0], ENTITY_NONE), 1);
        assert_eq!(world.point_contents([100.0, 0.0, 0.0], ENTITY_NONE), 0);
    }

    #[test]
    fn source_point_trace_hits_cube() {
        let mut world = CollisionWorld::from_source_brushes(vec![cube()]).unwrap();
        let trace = world.trace(TraceQuery {
            start: [128.0, 0.0, 0.0],
            mins: [0.0; 3],
            maxs: [0.0; 3],
            end: [0.0, 0.0, 0.0],
            pass_entity: ENTITY_NONE,
            mask: 1,
        });
        assert!(trace.fraction > 0.49 && trace.fraction < 0.51);
        assert_eq!(trace.normal, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn source_box_trace_expands_wall() {
        let mut world = CollisionWorld::from_source_brushes(vec![cube()]).unwrap();
        let trace = world.trace(TraceQuery {
            start: [128.0, 0.0, 0.0],
            mins: [-16.0, -16.0, -24.0],
            maxs: [16.0, 16.0, 32.0],
            end: [0.0, 0.0, 0.0],
            pass_entity: ENTITY_NONE,
            mask: 1,
        });
        // Expanded +X plane is x=80; CM's 1/8-unit clip epsilon leaves us just outside it.
        assert!(trace.end[0] > 79.9 && trace.end[0] < 80.2, "{:?}", trace.end);
    }

    #[test]
    fn source_backend_still_traces_encoded_bbox_entities() {
        let mut world = CollisionWorld::from_source_brushes(vec![cube()]).unwrap();
        let trace = world.trace_entity(
            TraceQuery {
                start: [200.0, 0.0, 0.0],
                mins: [-16.0; 3],
                maxs: [16.0; 3],
                end: [100.0, 0.0, 0.0],
                pass_entity: ENTITY_NONE,
                mask: SOURCE_CONTENTS_BODY,
            },
            EntityClip::Box {
                mins: [-16.0; 3],
                maxs: [16.0; 3],
                origin: [100.0, 0.0, 0.0],
            },
        );
        assert!(trace.fraction < 1.0);
    }
}

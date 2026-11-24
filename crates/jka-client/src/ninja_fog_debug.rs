use glam::{Vec2, Vec3, Vec4};
use jka_movement::{CollisionWorld, ENTITY_NONE, TraceQuery, TraceWorld};
use jka_ninja::{FogDesc, FogPreset};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

static ENABLED: AtomicBool = AtomicBool::new(false);
static PLACED: AtomicBool = AtomicBool::new(false);
static PLACEMENT: Mutex<Option<(String, Placement)>> = Mutex::new(None);

pub(crate) const CM_PER_MAP_UNIT: f32 = 2.54;
// SurfaceAlignedVolumes uses a 200 m capture with 128 samples per axis.
const FVOL_EXTENT_CM: f32 = 20_000.0;
pub(crate) const SURFACE_SIZE: u32 = 128;

#[derive(Clone)]
pub(crate) struct Placement {
    pub(crate) base: Vec3,
    pub(crate) desc: FogDesc,
    pub(crate) preset: FogPreset,
    pub(crate) revision: u64,
    pub(crate) surface: Arc<FogSurface>,
    pub(crate) pawn: Option<Vec3>,
    last_capture: Option<(glam::IVec2, f32)>,
}

pub(crate) struct FogSurface {
    pub(crate) width: f32,
    pub(crate) centre_map: Vec2,
    pub(crate) planes: Vec<[f32; 4]>,
    pub(crate) vertical_bounds: [f32; 2],
    eye_z: f32,
}

impl FogSurface {
    // SampleHeightSimple's Landscape.GetHeight reads one surface per XY.
    // In a BSP, select the floor below the placement eye in each column.
    fn capture(world: &mut CollisionWorld, centre_map: Vec2, base_z: f32, eye_z: f32) -> Self {
        let width = FVOL_EXTENT_CM / CM_PER_MAP_UNIT;
        let size = SURFACE_SIZE as usize;
        let cell = width / SURFACE_SIZE as f32;
        let mut planes = Vec::with_capacity(size * size);
        let mut vertical_bounds = [0.0_f32; 2];
        for row in 0..SURFACE_SIZE {
            for col in 0..SURFACE_SIZE {
                let x = centre_map.x - width * 0.5 + (col as f32 + 0.5) * cell;
                let y = centre_map.y - width * 0.5 + (row as f32 + 0.5) * cell;
                let hit = floor_below(world, [x, y, eye_z]);
                planes.push(hit.map_or([0.0; 4], |hit| {
                    let relative = hit.height - base_z;
                    let max_height_change =
                        0.5 * cell * (hit.normal.x.abs() + hit.normal.y.abs()) / hit.normal.z;
                    vertical_bounds[0] = vertical_bounds[0].min(relative - max_height_change);
                    vertical_bounds[1] = vertical_bounds[1].max(relative + max_height_change);
                    hit.normal.extend(relative).to_array()
                }));
            }
        }
        Self { width, centre_map, planes, vertical_bounds, eye_z }
    }

    pub(crate) fn ground_at(&self, base: [f32; 3], position: [f32; 3]) -> Option<f32> {
        let xy = Vec2::new(position[0], position[1]);
        let uv = (xy - self.centre_map) / self.width + Vec2::splat(0.5);
        if uv.cmplt(Vec2::ZERO).any() || uv.cmpgt(Vec2::ONE).any() {
            return None;
        }
        let grid = uv * SURFACE_SIZE as f32;
        let cell = grid.floor().min(Vec2::splat((SURFACE_SIZE - 1) as f32));
        let plane = Vec4::from_array(self.planes[cell.y as usize * SURFACE_SIZE as usize + cell.x as usize]);
        if plane.z <= 0.0 {
            return None;
        }
        let offset = (grid - cell - Vec2::splat(0.5)) * (self.width / SURFACE_SIZE as f32);
        Some(base[2] + plane.w - plane.truncate().truncate().dot(offset) / plane.z)
    }
}

struct FloorHit {
    height: f32,
    normal: Vec3,
}

fn floor_below(world: &mut CollisionWorld, start: [f32; 3]) -> Option<FloorHit> {
    // OpenJK/TaystJK surfaceflags.h: SOLID | TERRAIN.
    let trace = world.trace(TraceQuery {
        start,
        end: [start[0], start[1], start[2] - 8192.0],
        mins: [0.0; 3],
        maxs: [0.0; 3],
        pass_entity: ENTITY_NONE,
        mask: 0x1 | 0x1000,
    });
    (trace.start_solid == 0 && trace.fraction < 1.0 && trace.normal[2] > 0.0)
        .then_some(FloorHit { height: trace.end[2], normal: Vec3::from_array(trace.normal) })
}

pub(crate) fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

pub(crate) fn wants_actors() -> bool {
    PLACED.load(Ordering::Relaxed)
}

pub(crate) fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::Relaxed);
    if !enabled {
        clear();
    }
}

pub(crate) fn spawn(
    map: String,
    start: [f32; 3],
    eye_z: f32,
    world: &mut CollisionWorld,
    desc: FogDesc,
    preset: FogPreset,
) -> bool {
    let Some(FloorHit { height: ground, .. }) = floor_below(world, start) else {
        return false;
    };
    let map_base = [start[0], start[1], ground];
    let base = Vec3::from_array(crate::scene::render_position(map_base));
    let surface = Arc::new(FogSurface::capture(world, Vec2::new(start[0], start[1]), ground, eye_z));
    static REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let revision = REVISION.fetch_add(1, Ordering::Relaxed);
    *PLACEMENT.lock().unwrap_or_else(PoisonError::into_inner) = Some((
        map,
        Placement {
            base,
            desc,
            preset,
            revision,
            surface,
            pawn: None,
            last_capture: None,
        },
    ));
    PLACED.store(true, Ordering::Relaxed);
    true
}

/// Landscape samplers have a single surface for each column. BSP columns can
/// contain a roof over the player's floor, so select the level from the pawn
/// and retain the capture while it covers that level and the moving volume.
pub(crate) fn refresh_mist_surface(map: &str, origin: Vec3, eye_height: f32, world: &mut CollisionWorld) {
    let mut stored = PLACEMENT.lock().unwrap_or_else(PoisonError::into_inner);
    let Some((stored_map, placement)) = stored.as_mut() else { return };
    if placement.preset != FogPreset::Mist || !stored_map.eq_ignore_ascii_case(map) {
        return;
    }
    let position = crate::scene::jka_position(origin.to_array());
    let base = crate::scene::jka_position(placement.base.to_array());
    let xy = Vec2::new(position[0], position[1]);
    let surface = &placement.surface;
    let half_sim = 0.5 * placement.desc.width_cm / CM_PER_MAP_UNIT;
    let leaves_capture = (xy - surface.centre_map).abs().max_element() + half_sim > surface.width * 0.5;
    let height = surface.ground_at(base, position);
    if !leaves_capture && position[2] <= surface.eye_z && height.is_some_and(|h| h <= position[2]) {
        return;
    }
    let Some(FloorHit { height: ground, .. }) = floor_below(world, position) else { return };
    if leaves_capture || ground > surface.eye_z || height.is_none_or(|h| h > position[2]) {
        let cell = surface.width / SURFACE_SIZE as f32;
        let key = ((xy / cell).floor().as_ivec2(), ground);
        if placement.last_capture == Some(key) {
            return;
        }
        let centre = if leaves_capture { xy } else { surface.centre_map };
        placement.surface = Arc::new(FogSurface::capture(world, centre, base[2], position[2] + eye_height));
        placement.last_capture = Some(key);
    }
}

pub(crate) fn clear() {
    if PLACED.swap(false, Ordering::Relaxed) {
        *PLACEMENT.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

pub(crate) fn placement(map: Option<&str>, pawn: Option<Vec3>) -> Option<Placement> {
    if !wants_actors() {
        return None;
    }
    PLACEMENT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|(stored_map, placement)| {
            stored_map.eq_ignore_ascii_case(map?).then(|| Placement {
                pawn: pawn.filter(|_| placement.preset == FogPreset::Mist),
                ..placement.clone()
            })
        })
}

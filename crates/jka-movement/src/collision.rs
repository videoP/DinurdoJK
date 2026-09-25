use super::*;
use jka_assets::bsp::{Bsp, SurfaceKind};

struct NativeWorld(NonNull<c_void>);
// CM globals and per-world checkcounts are serialized by the native recursive mutex.
unsafe impl Send for NativeWorld {}
unsafe impl Sync for NativeWorld {}
impl Drop for NativeWorld {
    fn drop(&mut self) {
        unsafe { ffi::jka_world_free(self.0.as_ptr()) };
    }
}

#[derive(Clone)]
pub struct CollisionWorld {
    native: Arc<NativeWorld>,
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
            native: Arc::new(NativeWorld(native)),
        })
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
        let mut result = TraceResult::clear(q.end);
        let ok = unsafe {
            match clip {
                EntityClip::InlineModel { index, origin, angles } => ffi::jka_world_trace(
                    self.native.0.as_ptr(),
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
                    self.native.0.as_ptr(),
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
        let contents = unsafe {
            ffi::jka_world_contents(self.native.0.as_ptr(), point.as_ptr(), index, origin.as_ptr(), angles.as_ptr())
        };
        assert_ne!(contents, -1, "{}", error());
        contents
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
        let mut result = TraceResult::clear(q.end);
        let zero = [0.0; 3];
        let ok = unsafe {
            ffi::jka_world_trace(
                self.native.0.as_ptr(),
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
        let zero = [0.0; 3];
        let contents = unsafe {
            ffi::jka_world_contents(
                self.native.0.as_ptr(),
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

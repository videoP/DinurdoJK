//! Bounded diagnostics for the real native movement solver.
use super::*;

/// Stages: 1 Force Jump assignment, 2 clip projection (including trial clips),
/// 3 actual slide collision, 4 final Pmove velocity, 5 slope correction,
/// 6 actual velocity snapping. Trial clips and step-slide retries are included.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct JumpTraceEvent {
    pub stage: i32,
    pub time: i32,
    pub msec: i32,
    pub flags: i32,
    pub ground: i32,
    pub walking: i32,
    pub ground_plane: i32,
    pub level: i32,
    pub active: i32,
    pub entity: i32,
    pub bump: i32,
    pub origin: [f32; 3],
    pub before: [f32; 3],
    pub after: [f32; 3],
    pub normal: [f32; 3],
    pub jump_start: f32,
    pub fraction: f32,
    pub overbounce: f32,
}

unsafe extern "C" {
    fn jka_jump_trace_begin(events: *mut JumpTraceEvent, capacity: i32);
    fn jka_jump_trace_end() -> i32;
}

impl PmoveContext {
    /// Captures at most 512 events for one command, using native Pmove and the
    /// caller's normal collision world. Diagnostics are off outside this call.
    pub fn trace_command<W: TraceWorld>(
        &self,
        state: &mut PlayerState,
        cmd: UserCmd,
        msec: i32,
        world: &mut W,
    ) -> Result<(PlayerView, Vec<JumpTraceEvent>), String> {
        if ffi::in_callback() {
            return Err("Cannot reenter Pmove inside a collision callback".into());
        }
        let _lock = ffi::NativeGuard::new();
        let mut events = vec![JumpTraceEvent::default(); 512];
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                unsafe {
                    jka_jump_trace_end();
                }
            }
        }
        unsafe {
            jka_jump_trace_begin(events.as_mut_ptr(), events.len() as i32);
        }
        let reset = Reset;
        let result = self.step_command_with_msec(state, cmd, msec, world);
        let count = unsafe { jka_jump_trace_end() } as usize;
        drop(reset);
        events.truncate(count);
        Ok((result?, events))
    }
}

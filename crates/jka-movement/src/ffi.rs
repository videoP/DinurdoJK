use super::*;
use std::{
    cell::Cell,
    ffi::c_char,
    panic::{catch_unwind, AssertUnwindSafe},
};

pub(crate) type TraceFn = unsafe extern "C" fn(
    *mut c_void,
    *mut TraceResult,
    *const f32,
    *const f32,
    *const f32,
    *const f32,
    i32,
    i32,
);
pub(crate) type ContentsFn = unsafe extern "C" fn(*mut c_void, *const f32, i32) -> i32;
unsafe extern "C" {
    pub fn jka_full_body_taunt(torso_anim: i32, legs_anim: i32) -> i32;
    pub fn jka_taunt_should_cancel(torso_anim: i32, legs_anim: i32, buttons: i32, upmove: i32) -> i32;
    pub fn jka_taunt_action_should_cancel(buttons: i32) -> i32;
    fn jka_lock();
    fn jka_unlock();
    #[cfg(test)]
    fn jka_contract(index: i32) -> i32;
    #[cfg(test)]
    fn japro_jka_contract(index: i32) -> i32;
    pub fn jka_load_animations(data: *const u8, length: i32) -> i32;
    pub fn jka_movement_error() -> *const c_char;
    pub fn jka_player_new(origin: *const f32, yaw: f32, spectator: i32) -> *mut c_void;
    pub fn jka_player_free(player: *mut c_void);
    pub fn jka_player_configure(player: *mut c_void, settings: *const PredictSettings) -> i32;
    pub fn jka_player_set_entities(player: *mut c_void, entities: *const PredictionEntity, count: i32) -> i32;
    pub fn jka_player_clip_entity(player: *const c_void, entity: *const PredictionEntity) -> i32;
    pub fn jka_player_clone(player: *const c_void) -> *mut c_void;
    pub fn jka_player_view(player: *const c_void, view: *mut PlayerView);
    pub fn jka_player_entity_view(player: *const c_void, view: *mut PlayerEntityView);
    pub fn jka_bg_g2_player_angles(
        input: *const PlayerAngleEntity,
        time: i32,
        lerp_origin: *const f32,
        lerp_angles: *const f32,
        frametime: i32,
        model_scale: *const f32,
        ci_legs: i32,
        ci_torso: i32,
        look_angles: *const f32,
        motion_matrix: *const f32,
        state: *mut PlayerAngleState,
        result: *mut PlayerAngleResult,
    ) -> i32;
    pub fn jka_player_jump_level(player: *mut c_void, level: i32);
    pub fn jka_player_knockback(player: *mut c_void, velocity: *const f32, duration: i32);
    pub fn jka_player_set_noclip(player: *mut c_void, enabled: i32);
    pub fn jka_player_teleport(player: *mut c_void, origin: *const f32, angles: *const f32, speed: i32);
    pub fn jka_player_give_all(player: *mut c_void);
    pub fn jka_player_set_saber_movement_info(
        player: *mut c_void,
        saber_num: i32,
        info: *const SaberMovementInfo,
    ) -> i32;
    pub fn jka_player_set_foot_bolts(player: *mut c_void, left: *const f32, right: *const f32) -> i32;
    pub fn jka_player_offline_force_tick(player: *mut c_void, time: i32, requested_power: i32);
    pub fn jka_animation_name(index: i32) -> *const c_char;
    pub fn jka_saber_move_trail_length(move_: i32) -> i32;
    pub fn jka_super_break_win_anim(anim: i32) -> i32;
    pub fn jka_item_count() -> i32;
    // Pure q_math.c helpers (no host state; no lock needed).
    pub fn ByteToDir(b: i32, dir: *mut f32);
    pub fn RotateAroundDirection(axis: *mut [f32; 3], yaw: f32);
    pub fn jka_item_pickup_sound(index: i32) -> *const c_char;
    pub fn jka_item_icon(index: i32) -> *const c_char;
    pub fn jka_item_info(
        index: i32,
        classname: *mut *const c_char,
        world_model: *mut *const c_char,
        world_model2: *mut *const c_char,
        view_model: *mut *const c_char,
        item_type: *mut i32,
        tag: *mut i32,
        quantity: *mut i32,
    ) -> i32;
    pub fn jka_player_step(
        player: *mut c_void,
        cmd: *const UserCmd,
        tick: i32,
        trace: TraceFn,
        contents: ContentsFn,
        context: *mut c_void,
    ) -> i32;
    pub fn jka_ps_field_count() -> i32;
    pub fn jka_player_set_network(
        player: *mut c_void,
        fields: *const i32,
        count: i32,
        stats: *const i32,
        persistant: *const i32,
        ammo: *const i32,
        powerups: *const i32,
    ) -> i32;
    pub fn jka_player_get_network(
        player: *const c_void,
        fields: *mut i32,
        count: i32,
        stats: *mut i32,
        persistant: *mut i32,
        ammo: *mut i32,
        powerups: *mut i32,
    ) -> i32;
    pub fn jka_player_update_view_angles(player: *mut c_void, cmd: *const UserCmd);
    pub fn jka_player_predict(
        player: *mut c_void,
        cmd: *const UserCmd,
        settings: *const PredictSettings,
        trace: TraceFn,
        contents: ContentsFn,
        context: *mut c_void,
    ) -> i32;
    pub fn jka_weapon_info(
        weapon: i32,
        ammo_index: *mut i32,
        energy_per_shot: *mut i32,
        alt_energy_per_shot: *mut i32,
    ) -> i32;
    pub fn jka_world_new(data: *const u8, length: i32) -> *mut c_void;
    pub fn jka_world_free(world: *mut c_void);
    pub fn jka_world_prepare_merge(world: *mut c_void) -> i32;
    pub fn jka_collision_merge_get() -> i32;
    pub fn jka_collision_merge_set(enabled: i32);
    pub fn jka_world_load_sub_bsp(
        world: *mut c_void,
        name: *const c_char,
        data: *const u8,
        length: i32,
    ) -> i32;
    pub fn jka_collision_error() -> *const c_char;
    pub fn jka_world_trace(
        world: *mut c_void,
        out: *mut TraceResult,
        start: *const f32,
        mins: *const f32,
        maxs: *const f32,
        end: *const f32,
        mask: i32,
        model: i32,
        origin: *const f32,
        angles: *const f32,
    ) -> i32;
    pub fn jka_world_contents(
        world: *mut c_void,
        point: *const f32,
        model: i32,
        origin: *const f32,
        angles: *const f32,
    ) -> i32;
    pub fn jka_world_trace_box_entity(
        world: *mut c_void,
        out: *mut TraceResult,
        start: *const f32,
        mins: *const f32,
        maxs: *const f32,
        end: *const f32,
        mask: i32,
        box_mins: *const f32,
        box_maxs: *const f32,
        origin: *const f32,
    ) -> i32;
}

#[test]
fn rust_bridge_layout_and_constants_match_original_headers() {
    let values = [
        size_of::<UserCmd>() as i32,
        size_of::<TraceResult>() as i32,
        size_of::<PlayerView>() as i32,
        i32::from(WP_SABER),
        PM_NOCLIP,
        PM_SPECTATOR,
        BUTTON_ATTACK,
        BUTTON_WALKING,
        BUTTON_ALT_ATTACK,
        ENTITY_WORLD,
        ENTITY_NONE,
        PMF_DUCKED,
        PMF_ROLLING,
        PMF_STUCK_TO_WALL,
        MovementPower::Speed as i32,
        MovementPower::Rage as i32,
        size_of::<PlayerEntityView>() as i32,
        size_of::<PlayerAngleEntity>() as i32,
        size_of::<PlayerAngleState>() as i32,
        size_of::<BoneAngleCommand>() as i32,
        size_of::<PlayerAngleResult>() as i32,
        size_of::<SaberMovementInfo>() as i32,
        size_of::<PredictSettings>() as i32,
        size_of::<PredictionEntity>() as i32,
    ];
    for (i, expected) in values.into_iter().enumerate() {
        assert_eq!(
            unsafe { jka_contract(i as i32) },
            expected,
            "bridge field {i}"
        );
        assert_eq!(unsafe { japro_jka_contract(i as i32) }, expected, "JAPRO bridge field {i}");
    }
}
// Recursive locking allows a TraceWorld callback to query native BSP collision.
// Reentering Pmove itself is forbidden because OpenJK's pm/pml are global.
pub(crate) struct NativeGuard(std::marker::PhantomData<*mut ()>);
impl NativeGuard {
    pub fn new() -> Self {
        unsafe { jka_lock() };
        Self(std::marker::PhantomData)
    }
}
impl Drop for NativeGuard {
    fn drop(&mut self) {
        unsafe { jka_unlock() };
    }
}
thread_local! { static IN_CALLBACK: Cell<bool> = const { Cell::new(false) }; }
pub(crate) struct Callbacks<'a, W> {
    world: &'a mut W,
    failure: Option<String>,
}
impl<'a, W> Callbacks<'a, W> {
    pub fn new(world: &'a mut W) -> Self {
        Self {
            world,
            failure: None,
        }
    }
    pub fn finish(self) -> Result<(), String> {
        if let Some(failure) = self.failure {
            Err(failure)
        } else {
            Ok(())
        }
    }
}
pub(crate) fn in_callback() -> bool {
    IN_CALLBACK.get()
}
unsafe fn vector(pointer: *const f32) -> [f32; 3] {
    unsafe { [*pointer, *pointer.add(1), *pointer.add(2)] }
}
pub(crate) unsafe extern "C" fn trace_callback<W: TraceWorld>(
    context: *mut c_void,
    out: *mut TraceResult,
    start: *const f32,
    mins: *const f32,
    maxs: *const f32,
    end: *const f32,
    pass: i32,
    mask: i32,
) {
    let callbacks = unsafe { &mut *context.cast::<Callbacks<'_, W>>() };
    let query = unsafe {
        TraceQuery {
            start: vector(start),
            mins: vector(mins),
            maxs: vector(maxs),
            end: vector(end),
            pass_entity: pass,
            mask,
        }
    };
    // Pmove can keep tracing after an error. Never call collision with NaNs,
    // and never feed the invalid start back into native movement as a hit.
    if callbacks.failure.is_some() || !query.is_finite() {
        callbacks.failure.get_or_insert_with(|| format!("Non-finite collision query: {query:?}"));
        unsafe { *out = TraceResult::blocked(query.start); }
        return;
    }
    IN_CALLBACK.set(true);
    let result = catch_unwind(AssertUnwindSafe(|| callbacks.world.trace(query)));
    IN_CALLBACK.set(false);
    let trace = match result {
        Ok(trace)
            if trace.fraction.is_finite()
                && (0.0..=1.0).contains(&trace.fraction)
                && (0..=ENTITY_NONE).contains(&trace.entity)
                && trace.distance.is_finite()
                && trace.end.iter().chain(&trace.normal).all(|v| v.is_finite()) =>
        {
            trace
        }
        other => {
            callbacks.failure = Some(match other {
                Ok(trace) => format!("Invalid collision result {trace:?} for query {query:?}"),
                Err(_) => format!("Collision callback panicked for query {query:?}"),
            });
            TraceResult::blocked(query.start)
        }
    };
    unsafe {
        *out = trace;
    }
}
pub(crate) unsafe extern "C" fn contents_callback<W: TraceWorld>(
    context: *mut c_void,
    point: *const f32,
    pass: i32,
) -> i32 {
    let callbacks = unsafe { &mut *context.cast::<Callbacks<'_, W>>() };
    let point = unsafe { vector(point) };
    if callbacks.failure.is_some() || !point.iter().all(|v| v.is_finite()) {
        callbacks.failure.get_or_insert_with(|| format!("Non-finite contents query: point={point:?} pass={pass}"));
        return 1; // CONTENTS_SOLID
    }
    IN_CALLBACK.set(true);
    let result = catch_unwind(AssertUnwindSafe(|| {
        callbacks
            .world
            .point_contents(point, pass)
    }));
    IN_CALLBACK.set(false);
    match result {
        Ok(contents) => contents,
        Err(_) => {
            callbacks.failure = Some(format!("Contents callback panicked: point={point:?} pass={pass}"));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CountingWorld { calls: usize, invalid_result: bool }
    impl TraceWorld for CountingWorld {
        fn trace(&mut self, query: TraceQuery) -> TraceResult {
            self.calls += 1;
            let mut result = TraceResult::clear(query.end);
            if self.invalid_result { result.normal[0] = f32::NAN; }
            result
        }
        fn point_contents(&mut self, _: [f32; 3], _: i32) -> i32 {
            self.calls += 1;
            0
        }
    }

    fn run_trace(callbacks: &mut Callbacks<'_, CountingWorld>, start: [f32; 3]) -> TraceResult {
        let zero = [0.0; 3];
        let mut result = TraceResult::clear(zero);
        unsafe {
            trace_callback::<CountingWorld>((callbacks as *mut Callbacks<'_, CountingWorld>).cast(),
                &mut result, start.as_ptr(), zero.as_ptr(), zero.as_ptr(), zero.as_ptr(), 0, 1);
        }
        result
    }

    #[test]
    fn invalid_queries_are_rejected_before_calling_world_and_latch_first_error() {
        let mut world = CountingWorld { calls: 0, invalid_result: false };
        let mut callbacks = Callbacks::new(&mut world);
        let result = run_trace(&mut callbacks, [f32::NAN; 3]);
        assert_eq!(result.fraction, 0.0);
        assert!(result.end.iter().all(|v| v.is_finite()));
        // Even valid subsequent calls are blocked until Pmove returns.
        run_trace(&mut callbacks, [0.0; 3]);
        let zero = [0.0; 3];
        let contents = unsafe {
            contents_callback::<CountingWorld>((&mut callbacks as *mut Callbacks<'_, CountingWorld>).cast(), zero.as_ptr(), 0)
        };
        assert_eq!(contents, 1);
        let error = callbacks.finish().unwrap_err();
        assert!(error.contains("Non-finite collision query"), "{error}");
        assert!(error.contains("NaN"), "{error}");
        assert_eq!(world.calls, 0);
        assert!(!in_callback());
    }

    #[test]
    fn invalid_contents_query_is_rejected_before_calling_world() {
        let mut world = CountingWorld { calls: 0, invalid_result: false };
        let mut callbacks = Callbacks::new(&mut world);
        let point = [f32::INFINITY; 3];
        let contents = unsafe {
            contents_callback::<CountingWorld>((&mut callbacks as *mut Callbacks<'_, CountingWorld>).cast(), point.as_ptr(), 2)
        };
        assert_eq!(contents, 1);
        assert!(callbacks.finish().unwrap_err().contains("Non-finite contents query"));
        assert_eq!(world.calls, 0);
    }

    #[test]
    fn invalid_callback_result_is_replaced_with_a_finite_blocked_trace() {
        let mut world = CountingWorld { calls: 0, invalid_result: true };
        let mut callbacks = Callbacks::new(&mut world);
        let result = run_trace(&mut callbacks, [1.0, 2.0, 3.0]);
        assert_eq!(result.end, [1.0, 2.0, 3.0]);
        assert_eq!(result.fraction, 0.0);
        assert!(callbacks.finish().unwrap_err().contains("Invalid collision result"));
        assert_eq!(world.calls, 1);
    }
}

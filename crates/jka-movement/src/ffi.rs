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
    fn jka_lock();
    fn jka_unlock();
    #[cfg(test)]
    fn jka_contract(index: i32) -> i32;
    pub fn jka_load_animations(data: *const u8, length: i32) -> i32;
    pub fn jka_movement_error() -> *const c_char;
    pub fn jka_player_new(origin: *const f32, yaw: f32, spectator: i32) -> *mut c_void;
    pub fn jka_player_free(player: *mut c_void);
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
    pub fn jka_player_set_saber_movement_info(
        player: *mut c_void,
        saber_num: i32,
        info: *const SaberMovementInfo,
    ) -> i32;
    pub fn jka_player_offline_force_tick(player: *mut c_void, time: i32, requested_power: i32);
    pub fn jka_animation_name(index: i32) -> *const c_char;
    pub fn jka_saber_move_trail_length(move_: i32) -> i32;
    pub fn jka_super_break_win_anim(anim: i32) -> i32;
    pub fn jka_item_count() -> i32;
    // Pure q_math.c helpers (no host state; no lock needed).
    pub fn ByteToDir(b: i32, dir: *mut f32);
    pub fn RotateAroundDirection(axis: *mut [f32; 3], yaw: f32);
    pub fn jka_item_info(
        index: i32,
        classname: *mut *const c_char,
        world_model: *mut *const c_char,
        world_model2: *mut *const c_char,
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
    ];
    for (i, expected) in values.into_iter().enumerate() {
        assert_eq!(
            unsafe { jka_contract(i as i32) },
            expected,
            "bridge field {i}"
        );
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
    failed: bool,
}
impl<'a, W> Callbacks<'a, W> {
    pub fn new(world: &'a mut W) -> Self {
        Self {
            world,
            failed: false,
        }
    }
    pub fn finish(self) -> Result<(), String> {
        if self.failed {
            Err("Collision callback failed".into())
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
        _ => {
            callbacks.failed = true;
            let mut t = TraceResult::clear(query.start);
            t.all_solid = 1;
            t.start_solid = 1;
            t.fraction = 0.0;
            t
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
    IN_CALLBACK.set(true);
    let result = catch_unwind(AssertUnwindSafe(|| {
        callbacks
            .world
            .point_contents(unsafe { vector(point) }, pass)
    }));
    IN_CALLBACK.set(false);
    match result {
        Ok(contents) => contents,
        Err(_) => {
            callbacks.failed = true;
            1
        }
    }
}

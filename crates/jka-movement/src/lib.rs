//! Original OpenJK multiplayer movement behind a serialized, engine-independent host.
//! All coordinates and angles are in JKA space (Z up, degrees). Full playerState_t
//! storage stays native, including animation, Force, weapon and event state.
mod collision;
mod ffi;
pub use collision::{CollisionWorld, EntityClip, SourceCollisionBrush, SourceCollisionPlane};
use std::{
    ffi::{c_void, CStr},
    ptr::NonNull,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

/// One `bg_itemlist` entry from the pinned OpenJK tables (gitem_t subset).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemInfo {
    pub classname: String,
    /// `world_model[0]`: MD3 or Ghoul2 GLM qpath (empty for none).
    pub world_model: String,
    /// `world_model[1]`: e.g. the `_pu.md3` pickup model CG_RegisterItemVisuals
    /// uses for thermal/trip mine/det pack (empty for none).
    pub world_model2: String,
    /// `view_model`: first-person weapon MD3 qpath (empty for none).
    pub view_model: String,
    /// itemType_t (IT_WEAPON = 1, IT_AMMO, IT_ARMOR, IT_HEALTH, IT_POWERUP,
    /// IT_HOLDABLE, IT_PERSISTANT_POWERUP, IT_TEAM).
    pub item_type: i32,
    pub tag: i32,
    pub quantity: i32,
}

/// `bg_itemlist[index]`; entity `modelindex` of an ET_ITEM is this index.
pub fn bg_item(index: i32) -> Option<ItemInfo> {
    let mut classname = std::ptr::null();
    let mut world_model = std::ptr::null();
    let mut world_model2 = std::ptr::null();
    let mut view_model = std::ptr::null();
    let (mut item_type, mut tag, mut quantity) = (0, 0, 0);
    // SAFETY: bg_itemlist is immutable static C data; the returned strings
    // are static literals (never freed) and are copied before returning.
    unsafe {
        if ffi::jka_item_info(index, &mut classname, &mut world_model, &mut world_model2, &mut view_model, &mut item_type, &mut tag, &mut quantity) == 0 {
            return None;
        }
        Some(ItemInfo {
            classname: CStr::from_ptr(classname).to_string_lossy().into_owned(),
            world_model: CStr::from_ptr(world_model).to_string_lossy().into_owned(),
            world_model2: CStr::from_ptr(world_model2).to_string_lossy().into_owned(),
            view_model: CStr::from_ptr(view_model).to_string_lossy().into_owned(),
            item_type,
            tag,
            quantity,
        })
    }
}

/// `bg_itemlist[index].pickup_sound` qpath (CG_EntityEvent EV_ITEM_PICKUP).
pub fn bg_item_pickup_sound(index: i32) -> Option<String> {
    // SAFETY: returns a static literal or "" for out-of-range indices.
    let sound = unsafe { CStr::from_ptr(ffi::jka_item_pickup_sound(index)) }.to_string_lossy().into_owned();
    (!sound.is_empty()).then_some(sound)
}

/// `bg_itemlist[index].icon` qpath (the HUD pickup icon); `None` for items without one.
pub fn bg_item_icon(index: i32) -> Option<String> {
    // SAFETY: returns a static literal or "" for out-of-range indices.
    let icon = unsafe { CStr::from_ptr(ffi::jka_item_icon(index)) }.to_string_lossy().into_owned();
    (!icon.is_empty()).then_some(icon)
}

/// q_math ByteToDir: the event normal encoding used by EV_MISSILE_* etc.
pub fn byte_to_dir(b: i32) -> [f32; 3] {
    let mut dir = [0.0f32; 3];
    // SAFETY: ByteToDir only reads its static table and writes 3 floats.
    unsafe { ffi::ByteToDir(b, dir.as_mut_ptr()) };
    dir
}

/// q_math RotateAroundDirection: axis[0] = `forward`, then an arbitrary
/// perpendicular rotated by `yaw` degrees and their cross product.
pub fn rotate_around_direction(forward: [f32; 3], yaw: f32) -> [[f32; 3]; 3] {
    let mut axis = [forward, [0.0; 3], [0.0; 3]];
    // SAFETY: operates only on the 3x3 matrix passed in.
    unsafe { ffi::RotateAroundDirection(axis.as_mut_ptr(), yaw) };
    axis
}

/// animTable name for an animation number (`BOTH_STAND1`, ...), if valid.
pub fn animation_name(index: i32) -> Option<&'static str> {
    // SAFETY: GetStringForID returns a pointer into the static animTable or null.
    unsafe {
        let name = ffi::jka_animation_name(index);
        (!name.is_null()).then(|| CStr::from_ptr(name).to_str().ok()).flatten()
    }
}

/// Exact `saberMoveData[move].trailLength` from the vendored OpenJK tables.
/// The renderer uses this instead of maintaining a second Rust copy.
pub fn saber_move_trail_length(move_: i32) -> i32 {
    unsafe { ffi::jka_saber_move_trail_length(move_) }.max(0)
}

/// Exact OpenJK `BG_SuperBreakWinAnim` classification.
pub fn super_break_win_anim(anim: i32) -> bool {
    unsafe { ffi::jka_super_break_win_anim(anim) != 0 }
}

/// `bg_numItems`.
pub fn bg_item_count() -> i32 {
    // SAFETY: reads an immutable C global.
    unsafe { ffi::jka_item_count() }
}

pub const ENTITY_WORLD: i32 = 1022;
pub const ENTITY_NONE: i32 = 1023;
pub const BUTTON_ATTACK: i32 = 1;
pub const BUTTON_WALKING: i32 = 16;
pub const BUTTON_ALT_ATTACK: i32 = 128;
/// OpenJK `GENCMD_SABERATTACKCYCLE` (`genCmds_t` in q_shared.h).
pub const GENCMD_SABERATTACKCYCLE: u8 = 26;
pub const WP_SABER: u8 = 3;
pub const TICK_MSEC: i32 = 8;
pub const MIN_TICK_MSEC: i32 = 1;
pub const MAX_TICK_MSEC: i32 = 33;
pub const PMF_DUCKED: i32 = 1;
pub const PMF_ROLLING: i32 = 4;
pub const PMF_STUCK_TO_WALL: i32 = 16384;
pub const PM_NOCLIP: i32 = 3;
pub const PM_SPECTATOR: i32 = 4;

/// Gameplay subset of OpenJK `saberInfo_t` consumed by shared BG/Pmove.
///
/// The renderer still owns hilt/blade presentation. This compact projection is
/// installed only while the native movement host runs, so local play receives
/// the same stance, movement-scale and saber restriction data as multiplayer.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SaberMovementInfo {
    pub present: i32,
    pub num_blades: i32,
    pub styles_learned: i32,
    pub styles_forbidden: i32,
    pub saber_flags: i32,
    pub move_speed_scale: f32,
    pub anim_speed_scale: f32,
    pub ready_anim: i32,
    pub draw_anim: i32,
    pub putaway_anim: i32,
    /// Non-zero for the local authority, which reserves the ownership token
    /// in `saberEntityNum`. Network prediction must leave the snapshot's
    /// value alone (see [`SaberMovementInfo::for_prediction`]).
    pub owns_entity_slot: i32,
    /// SFL2_NO_MANUAL_DEACTIVATE / SFL2_NO_MANUAL_DEACTIVATE2 (non-zero = set).
    pub no_manual_deactivate: i32,
    pub no_manual_deactivate2: i32,
    /// `bladeStyle2Start` and `singleBladeStyle` from the saber definition.
    pub blade_style2_start: i32,
    pub single_blade_style: i32,
}
impl Default for SaberMovementInfo {
    fn default() -> Self {
        Self {
            present: 0,
            num_blades: 1,
            styles_learned: 0,
            styles_forbidden: 0,
            saber_flags: 0,
            move_speed_scale: 1.0,
            anim_speed_scale: 1.0,
            ready_anim: -1,
            draw_anim: -1,
            putaway_anim: -1,
            owns_entity_slot: 1,
            no_manual_deactivate: 0,
            no_manual_deactivate2: 0,
            blade_style2_start: 0,
            single_blade_style: 0,
        }
    }
}
impl SaberMovementInfo {
    /// Stock `WP_SaberSetDefaults` gameplay state for an equipped saber.
    pub fn equipped_default() -> Self {
        Self { present: 1, ..Self::default() }
    }

    /// The `cgs.clientinfo[].saber[]` data CG_PredictPlayerState's Pmove reads
    /// through BG_MySaber. The snapshot playerState stays authoritative.
    pub fn for_prediction(self) -> Self {
        Self { owns_entity_slot: 0, ..self }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JoinMode {
    Player,
    #[default]
    Spectator,
}
#[derive(Debug, Clone, Copy)]
#[repr(i32)]
pub enum MovementPower {
    Speed = 2,
    Rage = 8,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UserCmd {
    pub server_time: i32,
    pub angles: [i32; 3],
    pub buttons: i32,
    pub weapon: u8,
    pub force_selection: u8,
    pub inventory_selection: u8,
    pub generic_command: u8,
    pub forward_move: i8,
    pub right_move: i8,
    pub up_move: i8,
}
impl Default for UserCmd {
    fn default() -> Self {
        Self {
            server_time: 0,
            angles: [0; 3],
            buttons: 0,
            weapon: WP_SABER,
            force_selection: 1,
            inventory_selection: 0,
            generic_command: 0,
            forward_move: 0,
            right_move: 0,
            up_move: 0,
        }
    }
}
/// How the native pmove rounds velocity after every step (`trap_SnapVector`): 0 OpenJK's
/// `Sys_SnapVector` (nearest, ties away from zero), 1 truncate, 2 floor, 3 nearest-even,
/// 4 none. Process-wide; the retail game code is the same everywhere but the engine's rounding
/// is not, and at 1 ms steps it decides friction and gravity.
pub fn set_snap_mode(mode: i32) {
    let _guard = ffi::NativeGuard::new();
    unsafe { ffi::jka_set_snap_mode(mode) };
}

pub fn angle_to_short(angle: f32) -> i32 {
    ((angle * (65536.0 / 360.0)) as i32) & 65535
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceResult {
    pub fraction: f32,
    pub end: [f32; 3],
    pub normal: [f32; 3],
    pub distance: f32,
    pub surface_flags: i32,
    pub contents: i32,
    pub entity: i32,
    pub start_solid: i32,
    pub all_solid: i32,
}
impl TraceResult {
    pub fn clear(end: [f32; 3]) -> Self {
        Self {
            fraction: 1.0,
            end,
            normal: [0.0; 3],
            distance: 0.0,
            surface_flags: 0,
            contents: 0,
            entity: ENTITY_NONE,
            start_solid: 0,
            all_solid: 0,
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub struct TraceQuery {
    pub start: [f32; 3],
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub end: [f32; 3],
    pub pass_entity: i32,
    pub mask: i32,
}
pub trait TraceWorld {
    fn trace(&mut self, query: TraceQuery) -> TraceResult;
    fn point_contents(&mut self, point: [f32; 3], pass_entity: i32) -> i32;
}

/// Read-only projection of the full native player state and latest Pmove results.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct PlayerView {
    pub origin: [f32; 3],
    pub velocity: [f32; 3],
    pub view_angles: [f32; 3],
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
    pub command_time: i32,
    pub pm_type: i32,
    pub pm_flags: i32,
    pub pm_time: i32,
    pub ground_entity: i32,
    pub view_height: i32,
    pub bob_cycle: i32,
    pub legs_anim: i32,
    pub legs_timer: i32,
    pub torso_anim: i32,
    pub torso_timer: i32,
    pub water_level: i32,
    pub water_type: i32,
    pub force_power: i32,
    pub force_jump_level: i32,
    pub event_sequence: i32,
    pub events: [i32; 2],
    pub event_parms: [i32; 2],
    pub touch_count: i32,
    pub touches: [i32; 32],
    pub delta_angles: [i32; 3],
    pub health: i32,
    pub active_powers: i32,
    pub rage_recovery: i32,
    pub armor: i32,
    pub max_health: i32,
    pub force_power_max: i32,
    pub weapon: i32,
    pub ammo: i32,
    /// Host-only: final OpenJK PmoveSingle forced the view via PM_SetPMViewAngle.
    pub view_forced: i32,
}
pub const BONE_ANGLES_POSTMULT: i32 = 0x0002;
pub const G2_ORIGIN: i32 = 0;
pub const G2_POSITIVE_X: i32 = 1;
pub const G2_POSITIVE_Z: i32 = 2;
pub const G2_POSITIVE_Y: i32 = 3;
pub const G2_NEGATIVE_X: i32 = 4;
pub const G2_NEGATIVE_Z: i32 = 5;
pub const G2_NEGATIVE_Y: i32 = 6;
pub const MAX_BONE_ANGLE_COMMANDS: usize = 16;
pub const BONE_NAME_CAP: usize = 64;

/// Minimal `entityState_t` projection consumed by OpenJK `BG_G2PlayerAngles`.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct PlayerAngleEntity {
    pub number: i32,
    pub entity_type: i32,
    pub velocity: [f32; 3],
    pub movement_dir: i32,
    pub legs_anim: i32,
    pub torso_anim: i32,
    pub e_flags: i32,
    pub weapon: i32,
    pub ground_entity_num: i32,
    pub force_frame: i32,
    pub saber_move: i32,
    pub vehicle_num: i32,
    pub held_by_client: i32,
    pub other_entity_num2: i32,
}

/// Persistent `cent->pe` / `clientInfo_t` state threaded through the actual
/// vendored OpenJK `BG_G2PlayerAngles` implementation.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct PlayerAngleState {
    pub torso_yawing: i32,
    pub torso_pitching: i32,
    pub legs_yawing: i32,
    pub torso_yaw_angle: f32,
    pub torso_pitch_angle: f32,
    pub legs_yaw_angle: f32,
    pub corr_time: i32,
    pub look_time: i32,
    pub super_smooth_time: i32,
    pub last_head_angles: [f32; 3],
}
impl PlayerAngleState {
    /// Initial player-angle state. OpenJK `CG_ResetPlayerEntity` seeds the
    /// centity torso/legs swing angles from the entity raw yaw/pitch.
    pub fn reset_from_angles(angles: [f32; 3]) -> Self {
        let mut state = Self::default();
        state.reset_entity_swing_from_angles(angles);
        state
    }

    /// Port of the angle-state portion of OpenJK `CG_ResetPlayerEntity`.
    ///
    /// `cent->pe.legs` / `cent->pe.torso` are cleared and reseeded, while
    /// clientInfo-owned `corrTime`, `lookTime`, and `lastHeadAngles` survive
    /// the reset. Only `superSmoothTime` is explicitly cleared on clientInfo.
    pub fn reset_entity_swing_from_angles(&mut self, angles: [f32; 3]) {
        self.torso_yawing = 0;
        self.torso_pitching = 0;
        self.legs_yawing = 0;
        self.torso_yaw_angle = angles[1];
        self.torso_pitch_angle = angles[0];
        self.legs_yaw_angle = angles[1];
        self.super_smooth_time = 0;
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoneAngleCommand {
    pub bone_name: [u8; BONE_NAME_CAP],
    pub angles: [f32; 3],
    pub flags: i32,
    pub up: i32,
    pub right: i32,
    pub forward: i32,
}
impl Default for BoneAngleCommand {
    fn default() -> Self {
        Self {
            bone_name: [0; BONE_NAME_CAP],
            angles: [0.0; 3],
            flags: 0,
            up: 0,
            right: 0,
            forward: 0,
        }
    }
}
impl BoneAngleCommand {
    pub fn bone_name(&self) -> &str {
        let len = self.bone_name.iter().position(|&byte| byte == 0).unwrap_or(BONE_NAME_CAP);
        std::str::from_utf8(&self.bone_name[..len]).unwrap_or("")
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerAngleResult {
    pub legs_axis: [f32; 9],
    pub legs_angles: [f32; 3],
    pub tur_angles: [f32; 3],
    pub command_count: i32,
    pub commands: [BoneAngleCommand; MAX_BONE_ANGLE_COMMANDS],
}
impl Default for PlayerAngleResult {
    fn default() -> Self {
        Self {
            legs_axis: [0.0; 9],
            legs_angles: [0.0; 3],
            tur_angles: [0.0; 3],
            command_count: 0,
            commands: [BoneAngleCommand::default(); MAX_BONE_ANGLE_COMMANDS],
        }
    }
}
impl PlayerAngleResult {
    pub fn commands(&self) -> &[BoneAngleCommand] {
        let count = self.command_count.clamp(0, MAX_BONE_ANGLE_COMMANDS as i32) as usize;
        &self.commands[..count]
    }
    pub fn axis(&self) -> [[f32; 3]; 3] {
        [
            [self.legs_axis[0], self.legs_axis[1], self.legs_axis[2]],
            [self.legs_axis[3], self.legs_axis[4], self.legs_axis[5]],
            [self.legs_axis[6], self.legs_axis[7], self.legs_axis[8]],
        ]
    }
}

/// Calls the actual vendored OpenJK `BG_G2PlayerAngles`. Ghoul2 renderer calls
/// are captured into `PlayerAngleResult` so the Rust Ghoul2 evaluator can apply
/// the exact bone-angle commands generated by the stock cgame/game code.
pub fn bg_g2_player_angles(
    input: &PlayerAngleEntity,
    time: i32,
    lerp_origin: [f32; 3],
    lerp_angles: [f32; 3],
    frametime: i32,
    model_scale: [f32; 3],
    ci_legs: i32,
    ci_torso: i32,
    look_angles: [f32; 3],
    motion_matrix: Option<&[f32; 12]>,
    state: &mut PlayerAngleState,
) -> Result<PlayerAngleResult, String> {
    let _guard = ffi::NativeGuard::new();
    let mut result = PlayerAngleResult::default();
    let ok = unsafe {
        ffi::jka_bg_g2_player_angles(
            input,
            time,
            lerp_origin.as_ptr(),
            lerp_angles.as_ptr(),
            frametime,
            model_scale.as_ptr(),
            ci_legs,
            ci_torso,
            look_angles.as_ptr(),
            motion_matrix.map_or(std::ptr::null(), |matrix| matrix.as_ptr()),
            state,
            &mut result,
        )
    };
    if ok == 0 {
        let error = unsafe { CStr::from_ptr(ffi::jka_movement_error()) }
            .to_string_lossy()
            .into_owned();
        Err(if error.is_empty() { "OpenJK BG_G2PlayerAngles failed".into() } else { error })
    } else {
        Ok(result)
    }
}

/// Read-only result of OpenJK `BG_PlayerStateToEntityState`, plus the
/// playerState fields consumed by OpenJK's third-person view decision.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct PlayerEntityView {
    pub number: i32, pub entity_type: i32, pub client_num: i32,
    pub origin: [f32; 3], pub velocity: [f32; 3], pub angles: [f32; 3], pub speed: f32, pub origin2: [f32; 3],
    pub trickedentindex: i32, pub trickedentindex2: i32, pub trickedentindex3: i32, pub trickedentindex4: i32,
    pub force_frame: i32, pub emplaced_owner: i32, pub generic_enemy_index: i32, pub active_force_pass: i32, pub movement_dir: i32,
    pub legs_anim: i32, pub torso_anim: i32, pub legs_flip: i32, pub torso_flip: i32, pub e_flags: i32, pub e_flags2: i32,
    pub saber_in_flight: i32, pub saber_entity_num: i32, pub saber_move: i32, pub force_powers_active: i32,
    pub bolt1: i32, pub other_entity_num2: i32, pub saber_holstered: i32, pub event: i32, pub event_parm: i32,
    pub weapon: i32, pub ground_entity_num: i32, pub powerups: i32, pub loop_sound: i32, pub generic1: i32,
    pub modelindex2: i32, pub constant_light: i32, pub is_jedi_master: i32, pub time2: i32, pub fireflag: i32,
    pub held_by_client: i32, pub rag_attach: i32, pub model_scale: i32, pub broken_limbs: i32,
    pub has_look_target: i32, pub look_target: i32, pub custom_rgba: [i32; 4], pub vehicle_num: i32,
    pub pm_type: i32, pub view_height: i32, pub health: i32, pub dead_yaw: i32, pub team: i32, pub zoom_mode: i32,
    pub emplaced_index: i32, pub force_hand_extend: i32, pub falling_to_death: i32,
}

impl PlayerView {
    pub fn legs_animation_name(&self) -> String {
        let _guard = ffi::NativeGuard::new();
        unsafe {
            CStr::from_ptr(ffi::jka_animation_name(self.legs_anim))
                .to_string_lossy()
                .into_owned()
        }
    }
    pub fn torso_animation_name(&self) -> String {
        let _guard = ffi::NativeGuard::new();
        unsafe {
            CStr::from_ptr(ffi::jka_animation_name(self.torso_anim))
                .to_string_lossy()
                .into_owned()
        }
    }
    pub fn eye_origin(&self) -> [f32; 3] {
        let mut p = self.origin;
        p[2] += self.view_height as f32;
        p
    }
    pub fn grounded(&self) -> bool {
        self.ground_entity != ENTITY_NONE
    }
}
/// Server Pmove configuration consumed by `CG_PredictPlayerState`
/// (`pmove_fixed`/`pmove_msec`/`pmove_float` and the cgs.* serverinfo rules).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PredictSettings {
    pub pmove_fixed: i32,
    pub pmove_msec: i32,
    pub pmove_float: i32,
    pub gametype: i32,
    pub debug_melee: i32,
    pub step_slide_fix: i32,
    pub no_spec_move: i32,
    pub tracemask: i32,
    pub no_footsteps: i32,
    /// Native implementation: 0 = stock OpenJK, 1 = TaystJK shared BG/Pmove.
    /// The TaystJK backend is used for JA+ and jaPRO, and can also be selected
    /// for Base when advertised TaystJK feature flags require it.
    pub backend: i32,
    /// Server identity consumed by the TaystJK cgs.serverMod branches:
    /// 0 = Base/other, 1 = jaPRO, 2 = JA+. Keep this separate from backend.
    pub server_mod: i32,
    /// JA+ `jp_cinfo` (`cgs.cinfo`). jaPRO continues to use `jcinfo`.
    pub cinfo: i32,
    pub jcinfo: i32,
    pub jcinfo2: i32,
    pub taystjk_info: i32,
    pub dmflags: i32,
    pub hook_pull: i32,
    pub restricts: i32,
    /// TaystJK cgs.baseGame: Raven SDK/base semantics even when the gamename is
    /// unusual (sv_legacyGameAPI/g_saberWallDamageScale detection).
    pub base_game: i32,
    pub plugin_disable: i32,
    pub legacy_fixes: u32,
}

impl Default for PredictSettings {
    fn default() -> Self {
        Self {
            pmove_fixed: 0, pmove_msec: 8, pmove_float: 0, gametype: 0,
            debug_melee: 0, step_slide_fix: 1, no_spec_move: 0,
            tracemask: 0x1111, no_footsteps: 0, backend: 0, server_mod: 0,
            cinfo: 0, jcinfo: 0, jcinfo2: 0, taystjk_info: 0, dmflags: 0,
            hook_pull: 0, restricts: 0, base_game: 0, plugin_disable: 1536, legacy_fixes: 0,
        }
    }
}

/// A playerState_t in wire form: the stock playerStateFields slots in schema
/// order (float fields as raw IEEE bits) plus the four transmitted arrays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkPlayerState {
    pub fields: Vec<u32>,
    pub stats: [i32; 16],
    pub persistant: [i32; 16],
    pub ammo: [i32; 16],
    pub powerups: [i32; 16],
}

/// Number of playerStateFields slots compiled into the native bridge.
pub fn network_field_count() -> usize {
    unsafe { ffi::jka_ps_field_count() as usize }
}

/// `weaponData[weapon]`: (ammoIndex, energyPerShot, altEnergyPerShot).
pub fn weapon_info(weapon: i32) -> Option<(i32, i32, i32)> {
    let (mut ammo, mut energy, mut alt) = (0, 0, 0);
    // SAFETY: weaponData is immutable static C data.
    (unsafe { ffi::jka_weapon_info(weapon, &mut ammo, &mut energy, &mut alt) } != 0)
        .then_some((ammo, energy, alt))
}

pub struct PlayerState {
    raw: NonNull<c_void>,
}

/// Snapshot entity inputs used by native mod collision and movement helpers.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PredictionEntity {
    pub number: i32,
    pub entity_type: i32,
    pub model_index: i32,
    pub bolt1: i32,
    pub trajectory_type: i32,
    pub origin: [f32; 3],
    pub velocity: [f32; 3],
    pub angular_velocity: [f32; 3],
    pub legs_anim: i32,
    pub torso_anim: i32,
    pub saber_move: i32,
}
// Ownership is exclusive; native globals are always protected by NativeGuard.
unsafe impl Send for PlayerState {}
impl PlayerState {
    pub fn set_prediction_entities(&mut self, entities: &[PredictionEntity]) -> Result<(), String> {
        if entities.len() > 1024 || entities.iter().any(|e| !e.origin.iter().chain(&e.velocity).chain(&e.angular_velocity).all(|v| v.is_finite())) {
            return Err("Invalid prediction entities".into());
        }
        let _guard = ffi::NativeGuard::new();
        if unsafe { ffi::jka_player_set_entities(self.raw.as_ptr(), entities.as_ptr(), entities.len() as i32) } == 0 {
            return Err("Native prediction entities rejected".into());
        }
        Ok(())
    }

    pub fn clips_prediction_entity(&self, entity: &PredictionEntity) -> bool {
        let _guard = ffi::NativeGuard::new();
        unsafe { ffi::jka_player_clip_entity(self.raw.as_ptr(), entity) != 0 }
    }
    /// Install connection settings before loading a snapshot or updating angles.
    pub fn configure(&mut self, settings: &PredictSettings) -> Result<(), String> {
        let _guard = ffi::NativeGuard::new();
        if unsafe { ffi::jka_player_configure(self.raw.as_ptr(), settings) } == 0 {
            return Err("Native movement configuration rejected".into());
        }
        Ok(())
    }
    /// A native playerState_t loaded from a snapshot (`cg.predictedPlayerState = cg.snap->ps`).
    pub fn from_network(state: &NetworkPlayerState) -> Result<Self, String> {
        let mut player = Self::spawn([0.0; 3], 0.0, JoinMode::Spectator)?;
        player.set_network(state)?;
        Ok(player)
    }
    /// Replace the whole playerState_t with a snapshot playerState. Pmove's
    /// bounding box and water results are recomputed by the next Pmove.
    pub fn set_network(&mut self, state: &NetworkPlayerState) -> Result<(), String> {
        let _guard = ffi::NativeGuard::new();
        let ok = unsafe {
            ffi::jka_player_set_network(
                self.raw.as_ptr(),
                state.fields.as_ptr().cast(),
                state.fields.len() as i32,
                state.stats.as_ptr(),
                state.persistant.as_ptr(),
                state.ammo.as_ptr(),
                state.powerups.as_ptr(),
            )
        };
        if ok == 0 {
            return Err(format!(
                "network playerState has {} fields; bridge expects {}",
                state.fields.len(),
                network_field_count()
            ));
        }
        Ok(())
    }
    /// The current playerState_t in wire form.
    pub fn network(&self) -> NetworkPlayerState {
        let _guard = ffi::NativeGuard::new();
        let mut state = NetworkPlayerState {
            fields: vec![0; network_field_count()],
            stats: [0; 16],
            persistant: [0; 16],
            ammo: [0; 16],
            powerups: [0; 16],
        };
        unsafe {
            ffi::jka_player_get_network(
                self.raw.as_ptr(),
                state.fields.as_mut_ptr().cast(),
                state.fields.len() as i32,
                state.stats.as_mut_ptr(),
                state.persistant.as_mut_ptr(),
                state.ammo.as_mut_ptr(),
                state.powerups.as_mut_ptr(),
            )
        };
        state
    }
    pub fn set_force_jump_level(&mut self, level: u8) -> Result<(), String> {
        let _guard = ffi::NativeGuard::new();
        if level > 3 {
            return Err("Force jump level must be 0..3".into());
        }
        unsafe { ffi::jka_player_jump_level(self.raw.as_ptr(), i32::from(level)) };
        Ok(())
    }
    pub fn apply_knockback(&mut self, velocity: [f32; 3], duration_ms: i32) -> Result<(), String> {
        let _guard = ffi::NativeGuard::new();
        if !velocity.iter().all(|v| v.is_finite()) || !(0..=200).contains(&duration_ms) {
            return Err("Invalid knockback".into());
        }
        unsafe { ffi::jka_player_knockback(self.raw.as_ptr(), velocity.as_ptr(), duration_ms) };
        Ok(())
    }
    pub fn set_noclip(&mut self, enabled: bool) {
        let _guard = ffi::NativeGuard::new();
        unsafe { ffi::jka_player_set_noclip(self.raw.as_ptr(), i32::from(enabled)) };
    }
    /// Port of OpenJK `TeleportPlayer` (the `setviewpos` cheat): origin + 1 unit
    /// up, a 400 u/s push along the new view, a 160 ms knockback hold and a
    /// toggled EF_TELEPORT_BIT so presentation does not lerp.
    pub fn teleport(&mut self, origin: [f32; 3], angles: [f32; 3]) -> Result<(), String> {
        if !origin.iter().chain(angles.iter()).all(|v| v.is_finite()) {
            return Err("Non-finite teleport".into());
        }
        let _guard = ffi::NativeGuard::new();
        unsafe { ffi::jka_player_teleport(self.raw.as_ptr(), origin.as_ptr(), angles.as_ptr(), 0) };
        Ok(())
    }
    /// Port of OpenJK game-side `G_Give(ent, "all", ...)` for the local authority.
    pub fn give_all(&mut self) {
        let _guard = ffi::NativeGuard::new();
        unsafe { ffi::jka_player_give_all(self.raw.as_ptr()) };
    }
    /// Install one of the two saber definitions visible to stock BG/Pmove.
    /// This is state owned by the authoritative player host, not renderer data.
    pub fn set_saber_movement_info(
        &mut self,
        saber_num: usize,
        info: SaberMovementInfo,
    ) -> Result<(), String> {
        if saber_num >= 2
            || !info.move_speed_scale.is_finite()
            || !info.anim_speed_scale.is_finite()
            || !(1..=8).contains(&info.num_blades)
        {
            return Err("Invalid saber movement metadata".into());
        }
        let _guard = ffi::NativeGuard::new();
        let ok = unsafe {
            ffi::jka_player_set_saber_movement_info(
                self.raw.as_ptr(),
                saber_num as i32,
                &info,
            )
        };
        if ok == 0 {
            return Err("Native saber movement metadata rejected".into());
        }
        Ok(())
    }
    /// `*l_leg_foot` / `*r_leg_foot` in Ghoul2 model space, from the presented
    /// pose. This is what `pmove_t::ghoul2` gives the real cgame so Pmove can
    /// pick the slope stand anims (leg dangle); `None` disables that path.
    pub fn set_foot_bolts(&mut self, bolts: Option<[[f32; 3]; 2]>) -> Result<(), String> {
        let _guard = ffi::NativeGuard::new();
        let ok = unsafe {
            match &bolts {
                Some([left, right]) => ffi::jka_player_set_foot_bolts(self.raw.as_ptr(), left.as_ptr(), right.as_ptr()),
                None => ffi::jka_player_set_foot_bolts(self.raw.as_ptr(), std::ptr::null(), std::ptr::null()),
            }
        };
        if ok == 0 {
            return Err("Native foot bolts rejected".into());
        }
        Ok(())
    }
    /// Server-owned resource updates for the offline session, separate from prediction.
    pub fn offline_force_tick(
        &mut self,
        server_time: i32,
        power: Option<MovementPower>,
    ) -> Result<(), String> {
        self.offline_force_tick_with_msec(server_time, power, TICK_MSEC)
    }
    pub fn offline_force_tick_with_msec(
        &mut self,
        server_time: i32,
        power: Option<MovementPower>,
        tick_msec: i32,
    ) -> Result<(), String> {
        if !(MIN_TICK_MSEC..=MAX_TICK_MSEC).contains(&tick_msec)
            || server_time < 0
            || self.view().command_time.checked_add(tick_msec) != Some(server_time)
        {
            return Err("Invalid offline tick time".into());
        }
        let _guard = ffi::NativeGuard::new();
        unsafe {
            ffi::jka_player_offline_force_tick(
                self.raw.as_ptr(),
                server_time,
                power.map_or(-1, |p| p as i32),
            )
        };
        Ok(())
    }
    pub fn spawn(origin: [f32; 3], yaw: f32, mode: JoinMode) -> Result<Self, String> {
        if !origin.iter().all(|v| v.is_finite()) || !yaw.is_finite() {
            return Err("Non-finite spawn".into());
        }
        let _guard = ffi::NativeGuard::new();
        let raw = NonNull::new(unsafe {
            ffi::jka_player_new(origin.as_ptr(), yaw, i32::from(mode == JoinMode::Spectator))
        })
        .ok_or("Player allocation failed")?;
        Ok(Self { raw })
    }
    pub fn view(&self) -> PlayerView {
        let _guard = ffi::NativeGuard::new();
        let mut view = PlayerView::default();
        unsafe { ffi::jka_player_view(self.raw.as_ptr(), &mut view) };
        view
    }
    pub fn entity_view(&self) -> PlayerEntityView {
        let _guard = ffi::NativeGuard::new();
        let mut view = PlayerEntityView::default();
        unsafe { ffi::jka_player_entity_view(self.raw.as_ptr(), &mut view) };
        view
    }
}
impl Clone for PlayerState {
    fn clone(&self) -> Self {
        let _guard = ffi::NativeGuard::new();
        Self {
            raw: NonNull::new(unsafe { ffi::jka_player_clone(self.raw.as_ptr()) })
                .expect("Player allocation failed"),
        }
    }
}
impl Drop for PlayerState {
    fn drop(&mut self) {
        unsafe { ffi::jka_player_free(self.raw.as_ptr()) };
    }
}

static NEXT_ANIMATIONS: AtomicU64 = AtomicU64::new(1);
static ACTIVE_ANIMATIONS: AtomicU64 = AtomicU64::new(0);
struct Animations {
    id: u64,
    bytes: Vec<u8>,
}
#[derive(Clone)]
pub struct PmoveContext {
    animations: Arc<Animations>,
    spectator_only: bool,
}
impl PmoveContext {
    /// Uses the user's stock models/players/_humanoid/animation.cfg; timings affect physics.
    pub fn new(animation_cfg: &[u8]) -> Result<Self, String> {
        if ffi::in_callback() {
            return Err("Cannot initialize Pmove inside a collision callback".into());
        }
        if animation_cfg.is_empty() || animation_cfg.len() >= 59999 || animation_cfg.contains(&0) {
            return Err("Invalid humanoid animation.cfg".into());
        }
        let result = Self {
            animations: Arc::new(Animations {
                id: NEXT_ANIMATIONS.fetch_add(1, Ordering::Relaxed),
                bytes: animation_cfg.to_vec(),
            }),
            spectator_only: false,
        };
        let _guard = ffi::NativeGuard::new();
        result.activate()?;
        Ok(result)
    }
    /// Spectator Pmove does not consume humanoid animation timings.
    pub fn spectator() -> Self {
        let mut context = Self::new(b"// spectator has no humanoid animation transitions\n")
            .expect("Static spectator context");
        context.spectator_only = true;
        context
    }
    fn activate(&self) -> Result<(), String> {
        if ACTIVE_ANIMATIONS.load(Ordering::Relaxed) != self.animations.id {
            let ok = unsafe {
                ffi::jka_load_animations(
                    self.animations.bytes.as_ptr(),
                    self.animations.bytes.len() as i32,
                )
            };
            if ok == 0 {
                ACTIVE_ANIMATIONS.store(0, Ordering::Relaxed);
                return Err(native_error());
            }
            ACTIVE_ANIMATIONS.store(self.animations.id, Ordering::Relaxed);
        }
        Ok(())
    }
    /// Exactly one fixed command tick using the stock 8 ms default.
    pub fn step<W: TraceWorld>(
        &self,
        state: &mut PlayerState,
        cmd: UserCmd,
        world: &mut W,
    ) -> Result<PlayerView, String> {
        self.step_with_msec(state, cmd, TICK_MSEC, world)
    }
    /// Exactly one fixed command tick. Rendering never supplies a physics dt.
    pub fn step_with_msec<W: TraceWorld>(
        &self,
        state: &mut PlayerState,
        cmd: UserCmd,
        tick_msec: i32,
        world: &mut W,
    ) -> Result<PlayerView, String> {
        if ffi::in_callback() {
            return Err("Cannot reenter Pmove inside a collision callback".into());
        }
        if !(MIN_TICK_MSEC..=MAX_TICK_MSEC).contains(&tick_msec) {
            return Err(format!(
                "Pmove tick must be {MIN_TICK_MSEC}..={MAX_TICK_MSEC} milliseconds"
            ));
        }
        let _guard = ffi::NativeGuard::new();
        self.activate()?;
        let before = state.view();
        if cmd.weapon >= 19 || cmd.force_selection >= 18 {
            return Err("Invalid user command selection".into());
        }
        if self.spectator_only && before.pm_type != PM_SPECTATOR {
            return Err("Player movement requires stock humanoid animations".into());
        }
        if before.command_time.checked_add(tick_msec) != Some(cmd.server_time) {
            return Err(format!(
                "Pmove commands must advance by exactly {tick_msec} milliseconds"
            ));
        }
        let mut callbacks = ffi::Callbacks::new(world);
        let ok = unsafe {
            ffi::jka_player_step(
                state.raw.as_ptr(),
                &cmd,
                tick_msec,
                ffi::trace_callback::<W>,
                ffi::contents_callback::<W>,
                (&mut callbacks as *mut ffi::Callbacks<'_, W>).cast(),
            )
        };
        callbacks.finish()?;
        if ok == 0 {
            return Err(native_error());
        }
        Ok(state.view())
    }
}
impl PmoveContext {
    /// `PM_UpdateViewAngles(ps, cmd)`, applied by CG_PredictPlayerState to
    /// every command when pmove_fixed is set, even ones it does not replay.
    pub fn update_view_angles(&self, state: &mut PlayerState, cmd: UserCmd) {
        let _guard = ffi::NativeGuard::new();
        unsafe { ffi::jka_player_update_view_angles(state.raw.as_ptr(), &cmd) };
    }

    /// One replayed prediction command through stock `Pmove` (which splits it
    /// into PmoveSingle steps itself) using the server's Pmove settings.
    pub fn predict<W: TraceWorld>(
        &self,
        state: &mut PlayerState,
        cmd: UserCmd,
        settings: &PredictSettings,
        world: &mut W,
    ) -> Result<(), String> {
        if ffi::in_callback() {
            return Err("Cannot reenter Pmove inside a collision callback".into());
        }
        let _guard = ffi::NativeGuard::new();
        self.activate()?;
        let mut callbacks = ffi::Callbacks::new(world);
        let ok = unsafe {
            ffi::jka_player_predict(
                state.raw.as_ptr(),
                &cmd,
                settings,
                ffi::trace_callback::<W>,
                ffi::contents_callback::<W>,
                (&mut callbacks as *mut ffi::Callbacks<'_, W>).cast(),
            )
        };
        callbacks.finish()?;
        if ok == 0 {
            return Err(native_error());
        }
        Ok(())
    }
}

fn native_error() -> String {
    unsafe {
        CStr::from_ptr(ffi::jka_movement_error())
            .to_string_lossy()
            .into_owned()
    }
}

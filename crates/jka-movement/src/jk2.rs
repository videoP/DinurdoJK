//! Isolated pinned MVSDK Pmove host. Input/output are JK2 wire fields, never
//! JKA presentation fields or a Rust approximation of native physics.
use crate::{ffi, TraceResult, TraceWorld, UserCmd};
use std::{
    ffi::{c_char, c_void, CStr},
    ptr::NonNull,
    sync::OnceLock,
};

pub const SOURCE_REVISION: &str = "4c7c463638192aead1ff3de8a88cc5de266bf3a2";
pub mod diagnostics;
pub mod select;

#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    V102 = 2,
    V104 = 4,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub gameplay_version: Version,
    pub pmove_fixed: i32,
    pub pmove_msec: i32,
    pub gametype: i32,
    pub tracemask: i32,
    pub no_footsteps: i32,
    /// Match G_ClientThink's rounded timestamps and SpectatorThink's zeroed
    /// pmove_t timing flags. Default zero retains the CGame replay contract.
    pub spectator_server_timing: i32,
    /// `cg_predictItems` (CG_TouchItem).
    pub predict_items: i32,
}
impl Settings {
    pub fn new(gameplay_version: Version) -> Self {
        Self {
            gameplay_version,
            pmove_fixed: 0,
            pmove_msec: 8,
            gametype: 0,
            tracemask: unsafe { jk2_contract(11) },
            no_footsteps: 0,
            spectator_server_timing: 0,
            predict_items: 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
    pub version: Version,
    pub fields: Vec<u32>,
    pub stats: [i32; 16],
    pub persistant: [i32; 16],
    pub ammo: [i32; 16],
    pub powerups: [i32; 16],
}
impl State {
    pub fn zero(version: Version) -> Self {
        Self {
            version,
            fields: vec![0; field_names(version).len()],
            stats: [0; 16],
            persistant: [0; 16],
            ammo: [0; 16],
            powerups: [0; 16],
        }
    }
    pub fn field_bits(&self, name: &str) -> Option<u32> {
        field_names(self.version)
            .iter()
            .position(|&n| n == name)
            .and_then(|i| self.fields.get(i).copied())
    }
    pub fn set_field_bits(&mut self, name: &str, bits: u32) -> Result<(), String> {
        let i = field_names(self.version)
            .iter()
            .position(|&n| n == name)
            .ok_or_else(|| format!("Unknown JK2 state field {name}"))?;
        *self
            .fields
            .get_mut(i)
            .ok_or("JK2 field vector has the wrong length")? = bits;
        Ok(())
    }
}

/// CG_PredictPlayerState's saber-lock facing inputs for one CGame frame:
/// `cg.time`, `cg.snap->ps` and the opponent centity's lerped origin.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Frame {
    pub cg_time: i32,
    pub lock_time: i32,
    pub lock_enemy: i32,
    pub has_lock_origin: i32,
    pub snap_origin: [f32; 3],
    pub lock_origin: [f32; 3],
}

/// One raw JK2 wire entityState_t (wire identifiers of its version).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntityState {
    pub version: Version,
    pub number: i32,
    pub fields: Vec<u32>,
}
impl EntityState {
    pub fn zero(version: Version, number: i32) -> Self {
        Self {
            version,
            number,
            fields: vec![0; entity_field_names(version).len()],
        }
    }
    pub fn field_bits(&self, name: &str) -> Option<u32> {
        entity_field_names(self.version)
            .iter()
            .position(|&n| n == name)
            .and_then(|i| self.fields.get(i).copied())
    }
    pub fn set_field_bits(&mut self, name: &str, bits: u32) -> Result<(), String> {
        let i = entity_field_names(self.version)
            .iter()
            .position(|&n| n == name)
            .ok_or_else(|| format!("Unknown JK2 entity field {name}"))?;
        *self
            .fields
            .get_mut(i)
            .ok_or("JK2 entity field vector has the wrong length")? = bits;
        Ok(())
    }
}

pub fn entity_field_names(version: Version) -> &'static [&'static str] {
    static V102: OnceLock<Vec<&'static str>> = OnceLock::new();
    static V104: OnceLock<Vec<&'static str>> = OnceLock::new();
    let slot = match version {
        Version::V102 => &V102,
        Version::V104 => &V104,
    };
    slot.get_or_init(|| unsafe {
        (0..jk2_entity_field_count(version as i32))
            .map(|i| {
                CStr::from_ptr(jk2_entity_field_name(version as i32, i))
                    .to_str()
                    .expect("native ASCII field name")
            })
            .collect()
    })
}

/// One pinned MVSDK `bg_itemlist` entry, in native JK2 item index and tags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub info: crate::ItemInfo,
    pub icon: String,
    pub pickup_sound: String,
}

/// `bg_itemlist[index]` of the pinned JK2 SDK (1.02 and 1.04 share it).
pub fn item(index: i32) -> Option<Item> {
    let mut strings = [std::ptr::null::<c_char>(); 6];
    let (mut item_type, mut tag, mut quantity) = (0, 0, 0);
    // SAFETY: bg_itemlist is immutable static C data with static literals.
    unsafe {
        if jk2_item_info(index, strings.as_mut_ptr(), &mut item_type, &mut tag, &mut quantity) == 0 {
            return None;
        }
        let text = |i: usize| CStr::from_ptr(strings[i]).to_string_lossy().into_owned();
        Some(Item {
            info: crate::ItemInfo {
                classname: text(0),
                world_model: text(1),
                world_model2: text(2),
                view_model: text(3),
                item_type,
                tag,
                quantity,
            },
            icon: text(4),
            pickup_sound: text(5),
        })
    }
}

pub fn field_names(version: Version) -> &'static [&'static str] {
    static V102: OnceLock<Vec<&'static str>> = OnceLock::new();
    static V104: OnceLock<Vec<&'static str>> = OnceLock::new();
    let slot = match version {
        Version::V102 => &V102,
        Version::V104 => &V104,
    };
    slot.get_or_init(|| unsafe {
        (0..jk2_ps_field_count(version as i32))
            .map(|i| {
                CStr::from_ptr(jk2_ps_field_name(version as i32, i))
                    .to_str()
                    .expect("native ASCII field name")
            })
            .collect()
    })
}

pub struct Player {
    raw: NonNull<c_void>,
    version: Version,
}
// Exclusive state ownership. All MVSDK globals / host callbacks use the same
// native lock as collision and the existing JKA backends.
unsafe impl Send for Player {}
impl Player {
    pub fn new(version: Version, animations: &[u8], state: &State) -> Result<Self, String> {
        if ffi::in_callback() {
            return Err("Cannot create JK2 Pmove inside a collision callback".into());
        }
        let _guard = ffi::NativeGuard::new();
        if state.version != version {
            return Err("JK2 snapshot version mismatch".into());
        }
        if animations.is_empty() || animations.len() >= 39999 {
            return Err("Invalid MVSDK animation.cfg length".into());
        }
        for (i, expected) in [
            (0, size_of::<UserCmd>()),
            (1, size_of::<TraceResult>()),
            (2, size_of::<Settings>()),
            (15, size_of::<Frame>()),
        ] {
            if unsafe { jk2_contract(i) } != expected as i32 {
                return Err(format!("JK2 host ABI mismatch at contract {i}"));
            }
        }
        let raw = NonNull::new(unsafe {
            jk2_player_new(version as i32, animations.as_ptr(), animations.len() as i32)
        })
        .ok_or_else(native_error)?;
        let mut player = Self { raw, version };
        player.set_network(state)?;
        Ok(player)
    }
    pub fn try_clone(&self) -> Result<Self, String> {
        let _guard = ffi::NativeGuard::new();
        NonNull::new(unsafe { jk2_player_clone(self.raw.as_ptr()) })
            .map(|raw| Self {
                raw,
                version: self.version,
            })
            .ok_or("Unable to clone JK2 native state".into())
    }
    pub fn set_network(&mut self, state: &State) -> Result<(), String> {
        if ffi::in_callback() {
            return Err("Cannot load JK2 state inside a collision callback".into());
        }
        if state.version != self.version || state.fields.len() != field_names(self.version).len() {
            return Err("JK2 snapshot schema mismatch".into());
        }
        let _guard = ffi::NativeGuard::new();
        let ok = unsafe {
            jk2_player_set_network(
                self.raw.as_ptr(),
                state.fields.as_ptr(),
                state.fields.len() as i32,
                state.stats.as_ptr(),
                state.persistant.as_ptr(),
                state.ammo.as_ptr(),
                state.powerups.as_ptr(),
            )
        };
        if ok == 0 {
            return Err("JK2 snapshot contains invalid native movement fields".into());
        }
        Ok(())
    }
    pub fn network(&self) -> State {
        self.export(false)
    }
    /// MVSDK CGame internally uses 1.04 animation / block identifiers even on
    /// 1.02 servers. This is the native CGame state for presentation mapping.
    pub fn normalized(&self) -> State {
        self.export(true)
    }
    fn export(&self, normalized: bool) -> State {
        let _guard = ffi::NativeGuard::new();
        let mut state = State::zero(if normalized {
            Version::V104
        } else {
            self.version
        });
        let ok = unsafe {
            jk2_player_get_network(
                self.raw.as_ptr(),
                i32::from(normalized),
                state.fields.as_mut_ptr(),
                state.fields.len() as i32,
                state.stats.as_mut_ptr(),
                state.persistant.as_mut_ptr(),
                state.ammo.as_mut_ptr(),
                state.powerups.as_mut_ptr(),
            )
        };
        assert_ne!(ok, 0, "JK2 native export contract");
        state
    }
    pub fn predict<W: TraceWorld>(
        &mut self,
        cmd: UserCmd,
        settings: &Settings,
        world: &mut W,
    ) -> Result<(), String> {
        if ffi::in_callback() {
            return Err("Cannot reenter JK2 Pmove inside a collision callback".into());
        }
        validate_command(&cmd, settings)?;
        let _guard = ffi::NativeGuard::new();
        let mut callbacks = ffi::Callbacks::new(world);
        let ok = unsafe {
            jk2_player_predict(
                self.raw.as_ptr(),
                &cmd,
                settings,
                ffi::trace_callback::<W>,
                ffi::contents_callback::<W>,
                ffi::model_trace_callback::<W>,
                (&mut callbacks as *mut ffi::Callbacks<'_, W>).cast(),
            )
        };
        if let Err(error) = callbacks.finish() {
            unsafe { jk2_player_rollback(self.raw.as_ptr()) };
            return Err(error);
        }
        if ok == 0 {
            return Err(native_error());
        }
        Ok(())
    }
    /// CG_Init: forget every cg_entities client slot.
    pub fn reset_context(&mut self) {
        let _guard = ffi::NativeGuard::new();
        unsafe { jk2_context_reset(self.raw.as_ptr()) };
    }
    /// One CG_SetInitialSnapshot / CG_TransitionSnapshot: the snapshot's own
    /// playerstate becomes its client slot, then each client entity present
    /// replaces its slot. Absent clients keep their previous state. A failed
    /// import resets the whole context instead of leaving a partial snapshot.
    pub fn transition_context(&mut self, own: &State, entities: &[EntityState]) -> Result<(), String> {
        if ffi::in_callback() {
            return Err("Cannot load JK2 context inside a collision callback".into());
        }
        if own.version != self.version || own.fields.len() != field_names(self.version).len() {
            return Err("JK2 context snapshot schema mismatch".into());
        }
        let _guard = ffi::NativeGuard::new();
        let raw = self.raw.as_ptr();
        let mut ok = unsafe {
            jk2_context_set_self(
                raw,
                own.fields.as_ptr(),
                own.fields.len() as i32,
                own.stats.as_ptr(),
                own.persistant.as_ptr(),
                own.ammo.as_ptr(),
                own.powerups.as_ptr(),
            )
        } != 0;
        let mut failed = None;
        for entity in entities.iter().filter(|entity| (0..32).contains(&entity.number)) {
            if !ok {
                break;
            }
            ok = entity.version == self.version
                && unsafe {
                    jk2_context_set_entity(
                        raw,
                        entity.number,
                        entity.fields.as_ptr(),
                        entity.fields.len() as i32,
                    )
                } != 0;
            if !ok {
                failed = Some(entity.number);
            }
        }
        if ok {
            return Ok(());
        }
        unsafe { jk2_context_reset(raw) };
        Err(match failed {
            Some(number) => format!("JK2 client entity {number} has invalid native context fields"),
            None => "JK2 snapshot playerstate has invalid native context fields".into(),
        })
    }
    /// CG_TransitionEntity for the snapshot's non-client entities: trigger
    /// types (`triggers`, wire identifiers) replace their slot's currentState;
    /// every other transitioned slot number in `others` stops being a trigger
    /// candidate. `svflags` is the serverinfo `mvsdk_svFlags`.
    pub fn transition_world(&mut self, triggers: &[EntityState], others: &[i32], svflags: i32) -> Result<(), String> {
        if ffi::in_callback() {
            return Err("Cannot load JK2 context inside a collision callback".into());
        }
        let _guard = ffi::NativeGuard::new();
        let raw = self.raw.as_ptr();
        unsafe { jk2_context_clear_world_entities(raw, others.as_ptr(), others.len() as i32) };
        for entity in triggers {
            let ok = entity.version == self.version
                && unsafe {
                    jk2_context_set_world_entity(raw, entity.number, entity.fields.as_ptr(), entity.fields.len() as i32, svflags)
                } != 0;
            if !ok {
                unsafe { jk2_context_reset(raw) };
                return Err(format!("JK2 trigger entity {} has invalid native context fields", entity.number));
            }
        }
        Ok(())
    }
    /// CG_BuildSolidList's trigger list: the selected snapshot's entity
    /// numbers in snapshot order.
    pub fn set_triggers(&mut self, numbers: &[i32]) {
        let _guard = ffi::NativeGuard::new();
        unsafe { jk2_context_set_triggers(self.raw.as_ptr(), numbers.as_ptr(), numbers.len() as i32) };
    }
    /// `cg.hyperspace`: a committed command this frame touched a teleporter.
    pub fn hyperspace(&self) -> bool {
        let _guard = ffi::NativeGuard::new();
        unsafe { jk2_context_hyperspace(self.raw.as_ptr()) != 0 }
    }
    /// Entities whose currentState gained EF_NODRAW from a predicted pickup.
    pub fn predicted_nodraw(&self) -> Vec<i32> {
        let _guard = ffi::NativeGuard::new();
        let mut numbers = vec![0; 16];
        loop {
            let count = unsafe {
                jk2_context_predicted_nodraw(self.raw.as_ptr(), numbers.as_mut_ptr(), numbers.len() as i32)
            } as usize;
            if count <= numbers.len() {
                numbers.truncate(count);
                return numbers;
            }
            numbers.resize(count, 0);
        }
    }
    pub fn set_frame(&mut self, frame: &Frame) {
        let _guard = ffi::NativeGuard::new();
        unsafe { jk2_context_set_frame(self.raw.as_ptr(), frame) };
    }
    /// A client slot of the prediction-owned context, in native 1.04 IDs.
    pub fn context_entity(&self, number: i32) -> Option<EntityState> {
        let _guard = ffi::NativeGuard::new();
        let mut state = EntityState::zero(Version::V104, number);
        (unsafe {
            jk2_context_get_entity(
                self.raw.as_ptr(),
                number,
                state.fields.as_mut_ptr(),
                state.fields.len() as i32,
            )
        } != 0)
            .then_some(state)
    }
    /// Run one CGame frame's committed replay. Its remote animation/lock
    /// writebacks are kept only if every command succeeds; an error or panic
    /// restores the context exactly as it was before the frame.
    pub fn context_transaction<R>(
        &mut self,
        replay: impl FnOnce(&mut Self) -> Result<R, String>,
    ) -> Result<R, String> {
        {
            let _guard = ffi::NativeGuard::new();
            unsafe { jk2_context_mark(self.raw.as_ptr()) };
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| replay(self)));
        if !matches!(result, Ok(Ok(_))) {
            let _guard = ffi::NativeGuard::new();
            unsafe { jk2_context_restore_mark(self.raw.as_ptr()) };
        }
        match result {
            Ok(result) => result,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
    fn native_bytes(&self) -> Vec<u8> {
        let _guard = ffi::NativeGuard::new();
        let mut bytes = vec![0; unsafe { jk2_contract(3) } as usize];
        assert_ne!(
            unsafe { jk2_native_state(self.raw.as_ptr(), bytes.as_mut_ptr(), bytes.len() as i32) },
            0
        );
        bytes
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        let _guard = ffi::NativeGuard::new();
        unsafe { jk2_player_free(self.raw.as_ptr()) };
    }
}
fn validate_command(cmd: &UserCmd, settings: &Settings) -> Result<(), String> {
    if !(8..=33).contains(&settings.pmove_msec)
        || !(0..=1).contains(&settings.spectator_server_timing)
        || !(0..=8).contains(&settings.gametype)
        || cmd.angles.iter().any(|&v| !(-32768..=65535).contains(&v))
        || !(0..=i32::MAX - 1000).contains(&cmd.server_time)
        || i32::from(cmd.weapon) >= unsafe { jk2_contract(12) }
        || (cmd.force_selection != 255
            && i32::from(cmd.force_selection) >= unsafe { jk2_contract(13) })
        || (cmd.inventory_selection != 255
            && i32::from(cmd.inventory_selection) >= unsafe { jk2_contract(14) })
    {
        return Err("Invalid JK2 native command/settings".into());
    }
    Ok(())
}
fn native_error() -> String {
    let error = unsafe { CStr::from_ptr(jk2_movement_error()) }.to_string_lossy();
    if error.is_empty() {
        "MVSDK rejected the JK2 native operation".into()
    } else {
        error.into_owned()
    }
}

/// Compare command-by-command host calls with uninterrupted direct C Pmove.
/// Both paths use the pinned SDK and identical world inputs. This validates
/// the bridge/replay boundary, not an independent 32-bit server implementation.
pub fn verify_replay<W: TraceWorld>(
    seed: &Player,
    commands: &[UserCmd],
    settings: &Settings,
    mut world: impl FnMut() -> W,
) -> Result<usize, String> {
    if ffi::in_callback() {
        return Err("Cannot run a JK2 replay inside a collision callback".into());
    }
    if commands.len() > 10000 {
        return Err("JK2 diagnostic replay exceeds 10000 commands".into());
    }
    for cmd in commands {
        validate_command(cmd, settings)?;
    }
    let size = unsafe { jk2_contract(3) } as usize;
    let mut expected = vec![0; size * commands.len()];
    {
        let _guard = ffi::NativeGuard::new();
        let mut baseline_world = world();
        let mut callbacks = ffi::Callbacks::new(&mut baseline_world);
        let ok = unsafe {
            jk2_oracle_replay(
                seed.raw.as_ptr(),
                commands.as_ptr(),
                commands.len() as i32,
                settings,
                ffi::trace_callback::<W>,
                ffi::contents_callback::<W>,
                ffi::model_trace_callback::<W>,
                (&mut callbacks as *mut ffi::Callbacks<'_, W>).cast(),
                expected.as_mut_ptr(),
                size as i32,
            )
        };
        callbacks.finish()?;
        if ok == 0 {
            return Err(native_error());
        }
    }
    let mut player = seed.try_clone()?;
    let mut replay_world = world();
    for (i, cmd) in commands.iter().enumerate() {
        player
            .predict(*cmd, settings, &mut replay_world)
            .map_err(|e| format!("JK2 replay command {i} time {}: {e}", cmd.server_time))?;
        let actual = player.native_bytes();
        let expected = &expected[i * size..(i + 1) * size];
        if let Some(byte) = actual.iter().zip(expected).position(|(a, b)| a != b) {
            let word = byte / 4 * 4;
            let name = {
                let _guard = ffi::NativeGuard::new();
                unsafe { CStr::from_ptr(jk2_native_word_name(word as i32)) }
                    .to_string_lossy()
                    .into_owned()
            };
            return Err(format!("First JK2 replay divergence: command {i} time {} field {name} offset {word}: host={:02x?} oracle={:02x?}", cmd.server_time, &actual[word..word+4], &expected[word..word+4]));
        }
    }
    Ok(commands.len())
}

unsafe extern "C" {
    fn jk2_contract(index: i32) -> i32;
    fn jk2_ps_field_count(version: i32) -> i32;
    fn jk2_ps_field_name(version: i32, index: i32) -> *const c_char;
    fn jk2_player_new(version: i32, animations: *const u8, length: i32) -> *mut c_void;
    fn jk2_player_free(player: *mut c_void);
    fn jk2_player_clone(player: *const c_void) -> *mut c_void;
    fn jk2_player_set_network(
        player: *mut c_void,
        fields: *const u32,
        count: i32,
        stats: *const i32,
        persistant: *const i32,
        ammo: *const i32,
        powerups: *const i32,
    ) -> i32;
    fn jk2_player_get_network(
        player: *const c_void,
        normalized: i32,
        fields: *mut u32,
        count: i32,
        stats: *mut i32,
        persistant: *mut i32,
        ammo: *mut i32,
        powerups: *mut i32,
    ) -> i32;
    fn jk2_player_predict(
        player: *mut c_void,
        cmd: *const UserCmd,
        settings: *const Settings,
        trace: ffi::TraceFn,
        contents: ffi::ContentsFn,
        model_trace: ffi::ModelTraceFn,
        context: *mut c_void,
    ) -> i32;
    fn jk2_player_rollback(player: *mut c_void);
    fn jk2_item_info(
        index: i32,
        strings: *mut *const c_char,
        item_type: *mut i32,
        tag: *mut i32,
        quantity: *mut i32,
    ) -> i32;
    fn jk2_entity_field_count(version: i32) -> i32;
    fn jk2_entity_field_name(version: i32, index: i32) -> *const c_char;
    fn jk2_context_reset(player: *mut c_void);
    fn jk2_context_set_self(
        player: *mut c_void,
        fields: *const u32,
        count: i32,
        stats: *const i32,
        persistant: *const i32,
        ammo: *const i32,
        powerups: *const i32,
    ) -> i32;
    fn jk2_context_set_entity(player: *mut c_void, number: i32, fields: *const u32, count: i32) -> i32;
    fn jk2_context_get_entity(player: *const c_void, number: i32, fields: *mut u32, count: i32) -> i32;
    fn jk2_context_set_frame(player: *mut c_void, frame: *const Frame);
    fn jk2_context_mark(player: *mut c_void);
    fn jk2_context_restore_mark(player: *mut c_void);
    fn jk2_context_set_world_entity(player: *mut c_void, number: i32, fields: *const u32, count: i32, svflags: i32) -> i32;
    fn jk2_context_clear_world_entities(player: *mut c_void, numbers: *const i32, count: i32);
    fn jk2_context_set_triggers(player: *mut c_void, numbers: *const i32, count: i32);
    fn jk2_context_hyperspace(player: *const c_void) -> i32;
    fn jk2_context_predicted_nodraw(player: *const c_void, numbers: *mut i32, capacity: i32) -> i32;
    fn jk2_movement_error() -> *const c_char;
    fn jk2_native_state(player: *const c_void, out: *mut u8, size: i32) -> i32;
    fn jk2_native_word_name(offset: i32) -> *const c_char;
    fn jk2_oracle_replay(
        seed: *const c_void,
        commands: *const UserCmd,
        count: i32,
        settings: *const Settings,
        trace: ffi::TraceFn,
        contents: ffi::ContentsFn,
        model_trace: ffi::ModelTraceFn,
        context: *mut c_void,
        states: *mut u8,
        size: i32,
    ) -> i32;
}

#[cfg(test)]
mod tests;

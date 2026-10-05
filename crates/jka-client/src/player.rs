use crate::{
    camera::Camera,
    net::{PredictionWorld, SolidEntity},
    scene::{jka_position, render_position, SpawnPoint},
    surface_deformation::{LocalFootContactShim, SurfaceDeformationStamp},
};
use jka_movement::{
    angle_to_short, CollisionWorld, JoinMode, MovementPower, PlayerState, PlayerView,
    PmoveContext, SaberMovementInfo, TraceQuery, TraceResult, TraceWorld, UserCmd,
    BUTTON_WALKING, ENTITY_NONE, MAX_TICK_MSEC, MIN_TICK_MSEC, TICK_MSEC,
};
use std::{collections::HashSet, time::Duration};
use winit::keyboard::KeyCode;

const SHORT_TO_DEGREES: f32 = 360.0 / 65536.0;
// Stock OpenJK PM_UpdateViewAngles clamps PITCH to +/-16000 in 16-bit angle space.
// Despite the source comment calling this 90 degrees, the actual limit is 87.890625.
const JKA_MAX_VIEW_PITCH: f32 = 16000.0 * SHORT_TO_DEGREES;

/// TaystJK/OpenJK mouse-look cvars consumed after the platform layer has
/// produced relative mouse deltas. Raw input changes where the deltas come
/// from; it does not bypass sensitivity, yaw/pitch scale, or acceleration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MouseInputSettings {
    pub sensitivity: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub accel: f32,
}

impl Default for MouseInputSettings {
    fn default() -> Self {
        Self {
            sensitivity: 5.0,
            yaw: 0.022,
            pitch: 0.022,
            accel: 0.0,
        }
    }
}

impl MouseInputSettings {
    /// Sensitivity/acceleration-scaled mouse counts (before m_yaw/m_pitch),
    /// shared by the offline player and CL_MouseMove for live play.
    pub fn scale_mouse(self, mouse: (f64, f64), elapsed: Duration) -> (f32, f32) {
        self.scale_delta(mouse, elapsed)
    }

    /// Port of TaystJK's legacy `cl_mouseAccelStyle 0` scaling in
    /// `CL_MouseMove`. Runtime callers supply the time represented by the
    /// delta: the client-frame duration for accumulated input, or the raw-event
    /// interval for the low-latency subframe path.
    fn scale_delta(self, mouse: (f64, f64), elapsed: Duration) -> (f32, f32) {
        let mut mx = mouse.0 as f32;
        let mut my = mouse.1 as f32;
        if mx == 0.0 && my == 0.0 {
            return (0.0, 0.0);
        }

        let speed_msec = (elapsed.as_secs_f32() * 1000.0).max(1.0);
        let scale = if self.accel != 0.0 {
            let rate = (mx * mx + my * my).sqrt() / speed_msec;
            self.sensitivity + rate * self.accel
        } else {
            self.sensitivity
        };
        mx *= scale;
        my *= scale;
        (mx, my)
    }
}

fn delta_angle_degrees(delta: i32) -> f32 {
    // delta_angles uses Quake/JKA 16-bit circular angle arithmetic even though the
    // player-state field is an int.  OpenJK can therefore store values such as
    // -65335, which is +201 once wrapped to a signed 16-bit angle.
    (delta as i16) as f32 * SHORT_TO_DEGREES
}

fn idrive_axis(positive_down: bool, negative_down: bool, priority: i8, magnitude: i8) -> i8 {
    match (positive_down, negative_down) {
        (true, false) => magnitude,
        (false, true) => -magnitude,
        (true, true) => priority * magnitude,
        (false, false) => 0,
    }
}

/// Server-side game logic that shares the local player's fixed timeline: the
/// part of `G_RunFrame` and `ClientThink_real` that sits around each Pmove.
pub trait SoloGame {
    /// Brush models Pmove collides with, at the positions of the last frame.
    fn solids(&self) -> &[SolidEntity];
    /// `G_RunFrame` at `time` (the command time the coming Pmove ends on).
    /// `player` is the joined, clipping player; movers push it out of the way.
    fn run_frame(&mut self, time: i32, player: Option<&mut PlayerState>, world: &mut CollisionWorld);
    /// The rest of `ClientThink_real` after Pmove: touched entities, trigger
    /// contact and the use key. `player` is `None` unless it is a live, clipping player.
    fn after_pmove(&mut self, time: i32, player: Option<(&PlayerView, i32)>, world: &mut CollisionWorld);
}

/// A map with no game logic: nothing moves and nothing is solid but the world.
#[cfg(test)]
pub struct NoGame;

#[cfg(test)]
impl SoloGame for NoGame {
    fn solids(&self) -> &[SolidEntity] {
        &[]
    }
    fn run_frame(&mut self, _: i32, _: Option<&mut PlayerState>, _: &mut CollisionWorld) {}
    fn after_pmove(&mut self, _: i32, _: Option<(&PlayerView, i32)>, _: &mut CollisionWorld) {}
}

/// The static world plus the game's brush models, as Pmove traces them.
struct GameWorld<'a> {
    world: &'a mut World,
    solids: &'a [SolidEntity],
}
impl TraceWorld for GameWorld<'_> {
    fn trace(&mut self, q: TraceQuery) -> TraceResult {
        match self.world.0.as_mut() {
            Some(world) if !self.solids.is_empty() => {
                PredictionWorld { world, solids: self.solids, client_num: 0 }.trace(q)
            }
            Some(world) => world.trace(q),
            None => TraceResult::clear(q.end),
        }
    }
    fn point_contents(&mut self, point: [f32; 3], pass: i32) -> i32 {
        match self.world.0.as_mut() {
            Some(world) if !self.solids.is_empty() => {
                PredictionWorld { world, solids: self.solids, client_num: 0 }.point_contents(point, pass)
            }
            Some(world) => world.point_contents(point, pass),
            None => 0,
        }
    }
}

struct World(Option<CollisionWorld>);
impl TraceWorld for World {
    fn trace(&mut self, q: TraceQuery) -> TraceResult {
        self.0
            .as_mut()
            .map_or_else(|| TraceResult::clear(q.end), |world| world.trace(q))
    }
    fn point_contents(&mut self, point: [f32; 3], pass: i32) -> i32 {
        self.0
            .as_mut()
            .map_or(0, |world| world.point_contents(point, pass))
    }
}

pub struct LocalPlayer {
    movement: PmoveContext,
    world: World,
    can_join: bool,
    pub mode: JoinMode,
    player: Option<PlayerState>,
    spectator: PlayerState,
    accumulated: Duration,
    tick_msec: i32,
    input_angles: [f32; 3],
    // Usercmd angle basis that produced the current local Pmove state.
    // Subframe presentation advances from ps.viewangles by only mouse motion
    // accumulated since this command. Forced-view delta_angles are gameplay
    // state, not a presentation offset to replay after a saber lock.
    subframe_base_cmd_angles: [i32; 3],
    previous_eye: [f32; 3],
    current_eye: [f32; 3],
    previous_origin: [f32; 3],
    current_origin: [f32; 3],
    previous_command_time: i32,
    current_command_time: i32,
    pending_power: Option<MovementPower>,
    // CL_CreateCmd can run on a render frame where no fixed Pmove tick is due
    // (especially at the very high render rates this client supports). Keep
    // one-shot button/generic-command input alive until at least one Pmove
    // step consumes it, matching idTech's "wasPressed" behavior instead of
    // dropping a click between physics ticks.
    pending_cmd_buttons: i32,
    pending_generic_command: u8,
    // JKA cl_idrive 1 semantics: when opposite directions are both held,
    // the direction pressed most recently wins. Only one signed winner per
    // axis is needed, so this costs three bytes and needs no timestamp.
    move_priority: [i8; 3],
    noclip: bool,
    saber_movement: [SaberMovementInfo; 2],
    /// The viewer's foot bolts from its last posed frame (`pmove_t::ghoul2`).
    foot_bolts: Option<[[f32; 3]; 2]>,
    mouse_input: MouseInputSettings,
    deformation_shim: LocalFootContactShim,
}
impl LocalPlayer {
    pub fn new(
        movement: Option<PmoveContext>,
        world: Option<CollisionWorld>,
        spawn: SpawnPoint,
    ) -> Result<Self, String> {
        let can_join = movement.is_some() && world.is_some();
        let mut spectator = PlayerState::spawn(
            jka_position(spawn.position),
            spawn.yaw.to_degrees(),
            JoinMode::Spectator,
        )?;
        let saber_movement = [
            SaberMovementInfo::equipped_default(),
            SaberMovementInfo::default(),
        ];
        spectator.set_saber_movement_info(0, saber_movement[0])?;
        spectator.set_saber_movement_info(1, saber_movement[1])?;
        let spectator_view = spectator.view();
        let eye = spectator_view.eye_origin();
        Ok(Self {
            movement: movement.unwrap_or_else(PmoveContext::spectator),
            world: World(world),
            can_join,
            mode: JoinMode::Spectator,
            player: None,
            spectator,
            accumulated: Duration::ZERO,
            tick_msec: TICK_MSEC,
            input_angles: [0.0, spawn.yaw.to_degrees(), 0.0],
            subframe_base_cmd_angles: [
                angle_to_short(0.0),
                angle_to_short(spawn.yaw.to_degrees()),
                angle_to_short(0.0),
            ],
            previous_eye: eye,
            current_eye: eye,
            previous_origin: spectator_view.origin,
            current_origin: spectator_view.origin,
            previous_command_time: spectator_view.command_time,
            current_command_time: spectator_view.command_time,
            pending_power: None,
            pending_cmd_buttons: 0,
            pending_generic_command: 0,
            move_priority: [0; 3],
            noclip: false,
            saber_movement,
            foot_bolts: None,
            mouse_input: MouseInputSettings::default(),
            deformation_shim: LocalFootContactShim::new(spectator_view.origin),
        })
    }
    fn apply_saber_movement_to_state(
        state: &mut PlayerState,
        sabers: [SaberMovementInfo; 2],
    ) -> Result<(), String> {
        state.set_saber_movement_info(0, sabers[0])?;
        state.set_saber_movement_info(1, sabers[1])?;
        Ok(())
    }

    pub fn set_saber_movement_info(
        &mut self,
        sabers: [SaberMovementInfo; 2],
    ) -> Result<(), String> {
        Self::apply_saber_movement_to_state(&mut self.spectator, sabers)?;
        if let Some(player) = &mut self.player {
            Self::apply_saber_movement_to_state(player, sabers)?;
        }
        self.saber_movement = sabers;
        Ok(())
    }

    /// Foot bolts of the local player's presented Ghoul2 pose. Pmove needs
    /// them for the slope stand anims (leg dangle); applied before each step.
    pub fn set_foot_bolts(&mut self, bolts: Option<[[f32; 3]; 2]>) {
        self.foot_bolts = bolts;
    }

    pub fn can_join(&self) -> bool {
        self.can_join
    }

    /// Swap the static gameplay collision used by local Pmove without
    /// recreating the player/server state. Source-map edit previews build a
    /// fresh CollisionWorld, and keeping the existing LocalPlayer alive avoids
    /// resetting position, command time, saber state, or one-shot input.
    pub fn replace_collision_world(&mut self, world: CollisionWorld) {
        self.world.0 = Some(world);
    }
    pub fn set_noclip(&mut self, enabled: bool) -> Result<bool, String> {
        if self.mode != JoinMode::Player {
            if enabled {
                return Err("NOCLIP REQUIRES JOIN GAME MODE".into());
            }
            self.noclip = false;
            return Ok(false);
        }
        let player = self.player.as_mut().expect("joined player");
        player.set_noclip(enabled);
        self.noclip = enabled;
        self.pending_power = None;
        self.pause();
        Ok(enabled)
    }
    pub fn toggle_noclip(&mut self) -> Result<bool, String> {
        self.set_noclip(!self.noclip)
    }
    /// OpenJK `Cmd_SetViewpos_f` -> `TeleportPlayer` on the active state
    /// (joined player or free spectator). `origin` is in JKA world units.
    pub fn teleport(&mut self, origin: [f32; 3], angles: [f32; 3]) -> Result<(), String> {
        let state = match self.mode {
            JoinMode::Player => self.player.as_mut().expect("joined player"),
            JoinMode::Spectator => &mut self.spectator,
        };
        state.teleport(origin, angles)?;
        self.reset_view();
        Ok(())
    }
    pub fn give_all(&mut self) -> Result<(), String> {
        if self.mode != JoinMode::Player {
            return Err("GIVE REQUIRES JOIN GAME MODE".into());
        }
        self.player.as_mut().expect("joined player").give_all();
        Ok(())
    }
    pub fn view(&self) -> jka_movement::PlayerView {
        self.state().view()
    }
    pub fn entity_view(&self) -> jka_movement::PlayerEntityView {
        self.state().entity_view()
    }
    /// Authoritative local playerState in the same wire schema consumed by
    /// protocol snapshots. The local-server shim owns the conversion into the
    /// decoded protocol type so the player/Pmove layer stays protocol-agnostic.
    pub(crate) fn network_state(&self) -> jka_movement::NetworkPlayerState {
        self.state().network()
    }
    pub fn set_physics_tick_msec(&mut self, tick_msec: u32) -> Result<(), String> {
        let tick_msec = i32::try_from(tick_msec).map_err(|_| "Invalid physics tick")?;
        if !(MIN_TICK_MSEC..=MAX_TICK_MSEC).contains(&tick_msec) {
            return Err(format!(
                "Physics tick must be {MIN_TICK_MSEC}..={MAX_TICK_MSEC} milliseconds"
            ));
        }
        if self.tick_msec != tick_msec {
            self.tick_msec = tick_msec;
            self.pause();
        }
        Ok(())
    }
    fn state(&self) -> &PlayerState {
        match self.mode {
            JoinMode::Player => self.player.as_ref().expect("joined player"),
            JoinMode::Spectator => &self.spectator,
        }
    }
    pub fn join(&mut self, mode: JoinMode, spawn: SpawnPoint) -> Result<(), String> {
        if mode == JoinMode::Player && !self.can_join {
            return Err("JOIN GAME REQUIRES MAP COLLISION AND HUMANOID ANIMATIONS".into());
        }
        if mode != self.mode {
            if self.noclip {
                if let Some(player) = &mut self.player {
                    player.set_noclip(false);
                }
                self.noclip = false;
            }
            if mode == JoinMode::Player && self.player.is_none() {
                let mut player = PlayerState::spawn(
                    jka_position(spawn.position),
                    spawn.yaw.to_degrees(),
                    mode,
                )?;
                Self::apply_saber_movement_to_state(&mut player, self.saber_movement)?;
                self.player = Some(player);
            } else if mode == JoinMode::Spectator {
                let view = self.state().view();
                let mut spectator = PlayerState::spawn(view.origin, view.view_angles[1], mode)?;
                Self::apply_saber_movement_to_state(&mut spectator, self.saber_movement)?;
                self.spectator = spectator;
            }
            self.mode = mode;
        }
        self.reset_view();
        Ok(())
    }
    pub fn respawn(&mut self, spawn: SpawnPoint) -> Result<(), String> {
        let mut state = PlayerState::spawn(
            jka_position(spawn.position),
            spawn.yaw.to_degrees(),
            self.mode,
        )?;
        Self::apply_saber_movement_to_state(&mut state, self.saber_movement)?;
        if self.mode == JoinMode::Player && self.noclip {
            state.set_noclip(true);
        }
        match self.mode {
            JoinMode::Player => self.player = Some(state),
            JoinMode::Spectator => self.spectator = state,
        }
        self.reset_view();
        Ok(())
    }
    fn reset_view(&mut self) {
        let view = self.state().view();
        self.input_angles = std::array::from_fn(|i| {
            view.view_angles[i] - delta_angle_degrees(view.delta_angles[i])
        });
        // Match OpenJK cl.viewangles semantics: the command-angle accumulator is
        // not clamped to the gameplay pitch range. PM_UpdateViewAngles owns the
        // +/-16000 short-angle clamp and adjusts ps.delta_angles to preserve the
        // client command basis. Clamping here corrupts that relationship after
        // PM_SetPMViewAngle (saber locks, wall-run locks, etc.).
        self.input_angles[1] = self.input_angles[1].rem_euclid(360.0);
        self.subframe_base_cmd_angles = self.input_angles.map(angle_to_short);
        self.current_eye = view.eye_origin();
        self.previous_eye = self.current_eye;
        self.current_origin = view.origin;
        self.previous_origin = self.current_origin;
        self.current_command_time = view.command_time;
        self.previous_command_time = self.current_command_time;
        self.deformation_shim.reset(view.origin);
        self.pause();
    }
    pub fn pause(&mut self) {
        self.accumulated = Duration::ZERO;
        self.previous_eye = self.current_eye;
        self.previous_origin = self.current_origin;
        self.previous_command_time = self.current_command_time;
        self.pending_power = None;
        self.pending_cmd_buttons = 0;
        self.pending_generic_command = 0;
    }
    pub fn request_power(&mut self, power: MovementPower) {
        self.pending_power = Some(power);
    }
    pub(crate) fn note_movement_key_press(&mut self, key: KeyCode) {
        match key {
            KeyCode::KeyW => self.move_priority[0] = 1,
            KeyCode::KeyS => self.move_priority[0] = -1,
            KeyCode::KeyD => self.move_priority[1] = 1,
            KeyCode::KeyA => self.move_priority[1] = -1,
            KeyCode::Space => self.move_priority[2] = 1,
            KeyCode::ControlLeft | KeyCode::ControlRight => self.move_priority[2] = -1,
            _ => {}
        }
    }
    pub fn set_mouse_input_settings(&mut self, settings: MouseInputSettings) {
        self.mouse_input = settings;
    }

    pub fn apply_mouse_look_timed(&mut self, mouse: (f64, f64), elapsed: Duration) {
        let (mx, my) = self.mouse_input.scale_delta(mouse, elapsed);
        self.input_angles[1] =
            (self.input_angles[1] - self.mouse_input.yaw * mx).rem_euclid(360.0);
        // OpenJK accumulates mouse pitch into cl.viewangles without applying the
        // gameplay pitch clamp here. The resulting usercmd is clamped later by
        // PM_UpdateViewAngles, which also updates delta_angles. Keeping this raw
        // accumulator unbounded is required for forced-view delta compensation.
        self.input_angles[0] += self.mouse_input.pitch * my;
    }

    /// Convenience path used by unit tests without a timing sample. Runtime mouse
    /// input uses `apply_mouse_look_timed` so acceleration has a real timebase.
    #[cfg(test)]
    pub fn apply_mouse_look(&mut self, mouse: (f64, f64)) {
        self.apply_mouse_look_timed(mouse, Duration::from_millis(1));
    }

    #[cfg(test)]
    pub fn update(
        &mut self,
        elapsed: Duration,
        keys: &HashSet<KeyCode>,
        mouse: (f64, f64),
        client_cmd: UserCmd,
    ) -> Result<(), String> {
        self.update_with_game(elapsed, keys, mouse, client_cmd, &mut NoGame)
    }

    /// [`Self::update`] with server game logic running around every fixed step.
    pub fn update_with_game(
        &mut self,
        elapsed: Duration,
        keys: &HashSet<KeyCode>,
        mouse: (f64, f64),
        client_cmd: UserCmd,
        game: &mut dyn SoloGame,
    ) -> Result<(), String> {
        self.apply_mouse_look_timed(mouse, elapsed);
        // Preserve tapped +commands until a fixed Pmove step actually runs.
        // Held buttons are present in client_cmd again on later frames; ORing
        // here is specifically what protects short taps at >physics-rate FPS.
        self.pending_cmd_buttons |= client_cmd.buttons;
        if client_cmd.generic_command != 0 {
            self.pending_generic_command = client_cmd.generic_command;
        }
        self.accumulated += elapsed.min(Duration::from_millis(250));
        let tick = Duration::from_millis(self.tick_msec as u64);
        let mut first_step = true;
        while self.accumulated >= tick {
            self.accumulated -= tick;
            let move_priority = self.move_priority;
            let foot_bolts = self.foot_bolts;
            let state = match self.mode {
                JoinMode::Player => self.player.as_mut().expect("joined player"),
                JoinMode::Spectator => &mut self.spectator,
            };
            state.set_foot_bolts(foot_bolts)?;
            let walk = keys.contains(&KeyCode::ShiftLeft) || keys.contains(&KeyCode::ShiftRight);
            let magnitude = if walk { 64 } else { 127 };
            let crouch =
                keys.contains(&KeyCode::ControlLeft) || keys.contains(&KeyCode::ControlRight);
            // Reuse the same CL_CreateCmd button/select/generic-command payload
            // as remote play. Local movement keeps its idrive axis winner and
            // local subframe angle path, but action semantics must not diverge.
            let mut buttons = client_cmd.buttons;
            let generic_command = if first_step {
                buttons |= self.pending_cmd_buttons;
                if self.pending_generic_command != 0 {
                    self.pending_generic_command
                } else {
                    client_cmd.generic_command
                }
            } else {
                0
            };
            if walk {
                buttons |= BUTTON_WALKING;
            } else {
                buttons &= !BUTTON_WALKING;
            }
            let cmd = UserCmd {
                server_time: state.view().command_time + self.tick_msec,
                angles: self.input_angles.map(angle_to_short),
                buttons,
                weapon: client_cmd.weapon,
                force_selection: client_cmd.force_selection,
                inventory_selection: client_cmd.inventory_selection,
                generic_command,
                forward_move: idrive_axis(
                    keys.contains(&KeyCode::KeyW),
                    keys.contains(&KeyCode::KeyS),
                    move_priority[0],
                    magnitude,
                ),
                right_move: idrive_axis(
                    keys.contains(&KeyCode::KeyD),
                    keys.contains(&KeyCode::KeyA),
                    move_priority[1],
                    magnitude,
                ),
                up_move: idrive_axis(
                    keys.contains(&KeyCode::Space),
                    crouch,
                    move_priority[2],
                    127,
                ),
                ..UserCmd::default()
            };
            self.previous_eye = self.current_eye;
            self.previous_origin = self.current_origin;
            self.previous_command_time = self.current_command_time;
            // Only a joined player that clips is pushed, touches triggers or
            // collides with brush models; a spectator or noclipper flies through.
            let clipping = self.mode == JoinMode::Player && !self.noclip;
            if let Some(collision) = self.world.0.as_mut() {
                game.run_frame(cmd.server_time, clipping.then_some(&mut *state), collision);
            }
            state.offline_force_tick_with_msec(
                cmd.server_time,
                self.pending_power.take(),
                self.tick_msec,
            )?;
            let view = if clipping {
                let mut traced = GameWorld { world: &mut self.world, solids: game.solids() };
                self.movement.step_with_msec(state, cmd, self.tick_msec, &mut traced)?
            } else {
                self.movement
                    .step_with_msec(state, cmd, self.tick_msec, &mut self.world)?
            };
            if let Some(collision) = self.world.0.as_mut() {
                let alive = clipping && view.health > 0;
                game.after_pmove(cmd.server_time, alive.then_some((&view, cmd.buttons)), collision);
            }
            // The current Pmove state came from this exact command. Subframe
            // presentation uses it as the baseline, like remote prediction does.
            self.subframe_base_cmd_angles = cmd.angles;
            self.current_eye = view.eye_origin();
            self.current_origin = view.origin;
            self.current_command_time = view.command_time;
            if first_step {
                self.pending_cmd_buttons = 0;
                self.pending_generic_command = 0;
                first_step = false;
            }
            if self.mode == JoinMode::Player && !self.noclip {
                self.deformation_shim.observe_pmove(view, &mut self.world);
            }
        }
        Ok(())
    }

    pub fn take_deformation_stamps(&mut self) -> Vec<SurfaceDeformationStamp> {
        self.deformation_shim.take_pending()
    }
    pub fn autofocus_distance(&mut self, camera: &Camera, max_distance: f32) -> f32 {
        // Trace the same static BSP collision used by player movement straight
        // through the crosshair. This gives DOF a gameplay-friendly autofocus
        // target without a GPU readback or per-pixel center-depth sample.
        const CONTENTS_SOLID: i32 = 0x0000_0001;
        const CONTENTS_TERRAIN: i32 = 0x0000_1000;
        let max_distance = max_distance.max(64.0);
        let start = camera.position;
        let end = start + camera.forward() * max_distance;
        let result = self.world.trace(TraceQuery {
            start: jka_position(start.to_array()),
            mins: [0.0; 3],
            maxs: [0.0; 3],
            end: jka_position(end.to_array()),
            pass_entity: ENTITY_NONE,
            mask: CONTENTS_SOLID | CONTENTS_TERRAIN,
        });
        let hit = glam::Vec3::from_array(render_position(result.end));
        (hit - start).length().clamp(32.0, max_distance)
    }

    fn presentation_alpha(&self) -> f32 {
        let tick_seconds = self.tick_msec as f32 * 0.001;
        if tick_seconds <= 0.0 {
            0.0
        } else {
            (self.accumulated.as_secs_f32() / tick_seconds).clamp(0.0, 1.0)
        }
    }

    /// Fixed-step player origin interpolated on the presentation timeline. The
    /// returned coordinates remain in JKA/OpenJK axis order for the cgame path.
    pub fn presentation_origin(&self) -> [f32; 3] {
        let alpha = self.presentation_alpha();
        std::array::from_fn(|i| {
            self.previous_origin[i] + alpha * (self.current_origin[i] - self.previous_origin[i])
        })
    }

    /// Millisecond presentation clock aligned with `presentation_origin`. This
    /// is the OpenJK-style render/cgame time between the previous and current
    /// pmove samples, rather than the stepped playerState commandTime.
    pub fn presentation_time(&self) -> i32 {
        let alpha = self.presentation_alpha();
        let span = self
            .current_command_time
            .saturating_sub(self.previous_command_time)
            .max(0);
        self.previous_command_time
            .saturating_add((alpha * span as f32).round() as i32)
    }

    pub fn world_position(&self) -> glam::Vec3 {
        glam::Vec3::from_array(render_position(self.state().view().origin))
    }

    fn camera_position(&self) -> glam::Vec3 {
        let alpha = self.presentation_alpha();
        let eye = std::array::from_fn(|i| {
            self.previous_eye[i] + alpha * (self.current_eye[i] - self.previous_eye[i])
        });
        glam::Vec3::from_array(render_position(eye))
    }

    pub fn camera(&self, camera: &mut Camera) {
        camera.position = self.camera_position();
        let view = self.state().view();
        camera.yaw = view.view_angles[1].to_radians();
        camera.pitch = -view.view_angles[0].to_radians();
    }

    pub fn subframe_view_angles(&self) -> [f32; 3] {
        let view = self.state().view();
        if view.view_forced != 0 {
            // OpenJK owns the view for this Pmove state. Do not apply the
            // extra render/subframe mouse delta while it is locked.
            return view.view_angles;
        }

        // Start at OpenJK's Pmove result and add only mouse motion newer
        // than the usercmd that produced it. This prevents lock-time
        // PM_SetPMViewAngle compensation from being replayed on unlock.
        let since_pmove = |axis: usize| {
            let now = angle_to_short(self.input_angles[axis]);
            (now.wrapping_sub(self.subframe_base_cmd_angles[axis]) as i16) as f32
                * SHORT_TO_DEGREES
        };
        let pitch = (view.view_angles[0] + since_pmove(0))
            .clamp(-JKA_MAX_VIEW_PITCH, JKA_MAX_VIEW_PITCH);
        let yaw = view.view_angles[1] + since_pmove(1);
        [pitch, yaw, view.view_angles[2]]
    }

    pub fn camera_subframe(&self, camera: &mut Camera) {
        camera.position = self.camera_position();
        let angles = self.subframe_view_angles();
        camera.yaw = angles[1].to_radians();
        camera.pitch = -angles[0].to_radians();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[allow(dead_code)]
    mod support {
        include!("../../jka-movement/tests/support/mod.rs");
    }

    fn local() -> (LocalPlayer, SpawnPoint) {
        let spawn = SpawnPoint {
            position: render_position([0.0, 0.0, 24.125]),
            yaw: 0.0,
            ..SpawnPoint::default()
        };
        let mut player = LocalPlayer::new(
            Some(support::animations()),
            Some(support::Map::floor().world()),
            spawn,
        )
        .unwrap();
        player.join(JoinMode::Player, spawn).unwrap();
        (player, spawn)
    }
    #[test]
    fn idrive_axis_uses_the_most_recent_opposite_direction() {
        assert_eq!(idrive_axis(true, false, -1, 127), 127);
        assert_eq!(idrive_axis(false, true, 1, 127), -127);
        assert_eq!(idrive_axis(true, true, 1, 127), 127);
        assert_eq!(idrive_axis(true, true, -1, 127), -127);
        assert_eq!(idrive_axis(false, false, 1, 127), 0);
    }

    #[test]
    fn taystjk_mouse_defaults_apply_stock_point_eleven_degrees_per_count() {
        let settings = MouseInputSettings::default();
        let (mx, my) = settings.scale_delta((1.0, -1.0), Duration::from_millis(8));
        assert!((settings.yaw * mx - 0.11).abs() < 1.0e-6);
        assert!((settings.pitch * my + 0.11).abs() < 1.0e-6);
    }

    #[test]
    fn legacy_mouse_accel_matches_taystjk_style_zero_formula() {
        let settings = MouseInputSettings {
            sensitivity: 5.0,
            yaw: 0.022,
            pitch: 0.022,
            accel: 0.5,
        };
        let (mx, my) = settings.scale_delta((3.0, 4.0), Duration::from_millis(10));
        // rate = hypot(3,4)/10ms = 0.5; scale = 5 + 0.5*0.5 = 5.25.
        assert!((mx - 15.75).abs() < 1.0e-6);
        assert!((my - 21.0).abs() < 1.0e-6);
    }

    #[test]
    fn fixed_commands_are_independent_of_render_frequency() {
        let mut states = Vec::new();
        let keys = HashSet::from([KeyCode::KeyW, KeyCode::Space]);
        for fps in [30, 60, 125, 144, 240] {
            let (mut player, _) = local();
            let frames = fps * 2;
            for frame in 0..frames {
                let start = frame * 2_000_000_000u64 / frames;
                let end = (frame + 1) * 2_000_000_000u64 / frames;
                player
                    .update(
                        Duration::from_nanos(end - start),
                        &keys,
                        (0.0, 0.0),
                        UserCmd::default(),
                    )
                    .unwrap();
            }
            assert_eq!(player.state().view().command_time, 2000);
            states.push(player.state().view());
        }
        assert!(states.windows(2).all(|pair| pair[0] == pair[1]));
    }
    #[test]
    fn subframe_camera_uses_mouse_input_before_a_physics_tick() {
        let (mut player, _) = local();
        let mut camera = Camera::new([0.0, 0.0, 0.0], 0.0);
        player.camera(&mut camera);
        let before_yaw = camera.yaw;
        let before_time = player.state().view().command_time;

        player.apply_mouse_look((8.0, 0.0));
        player.camera_subframe(&mut camera);

        assert_ne!(camera.yaw, before_yaw);
        assert_eq!(player.state().view().command_time, before_time);
    }

    #[test]
    fn mouse_pitch_accumulator_stays_raw_while_pmove_clamps_the_view() {
        let (mut player, _) = local();

        // OpenJK CL_MouseMove accumulates cl.viewangles directly. A 1000-count
        // movement at the stock 5 * 0.022 scale is 110 degrees, beyond the
        // gameplay pitch limit; the client accumulator must still keep 110.
        player.apply_mouse_look((0.0, 1000.0));
        assert!((player.input_angles[0] - 110.0).abs() < 1.0e-4);

        // PM_UpdateViewAngles, not CL_MouseMove, applies the actual +/-16000
        // short-angle view limit and updates delta_angles accordingly.
        player
            .update(
                Duration::from_millis(TICK_MSEC as u64),
                &HashSet::new(),
                (0.0, 0.0),
                UserCmd::default(),
            )
            .unwrap();
        let view = player.state().view();
        assert!((view.view_angles[0] - JKA_MAX_VIEW_PITCH).abs() < 1.0e-3);
        assert!((player.input_angles[0] - 110.0).abs() < 1.0e-4);
    }

    #[test]
    fn subframe_view_is_relative_to_the_last_pmove_command() {
        let (mut player, _) = local();

        // Force PM_UpdateViewAngles to create a non-zero delta compensation.
        // With no newer mouse after that command, subframe presentation must
        // still be exactly the Pmove view, not input_angles + delta_angles.
        player.apply_mouse_look((0.0, 1000.0));
        player
            .update(
                Duration::from_millis(TICK_MSEC as u64),
                &HashSet::new(),
                (0.0, 0.0),
                UserCmd::default(),
            )
            .unwrap();
        let view = player.state().view();
        assert_ne!(view.delta_angles[0], 0);
        let subframe = player.subframe_view_angles();
        assert!((subframe[0] - view.view_angles[0]).abs() < 1.0e-4);
        assert!((subframe[1] - view.view_angles[1]).abs() < 1.0e-4);

        // Mouse newer than the Pmove command still advances presentation.
        player.apply_mouse_look((0.0, -10.0));
        let advanced = player.subframe_view_angles();
        assert_ne!(advanced[0], view.view_angles[0]);
    }

    #[test]
    fn wrapped_delta_angles_use_jka_short_semantics() {
        // This is the value OpenJK produces when a command around -89 degrees
        // gets clamped to -16000: -65335 is circularly the same as +201.
        let wrapped = delta_angle_degrees(-65_335);
        assert!((wrapped - 201.0 * SHORT_TO_DEGREES).abs() < 1.0e-6);
    }

    #[test]
    fn physics_tick_can_change_from_stock_8ms() {
        let (mut player, _) = local();
        player.set_physics_tick_msec(7).unwrap();
        let keys = HashSet::from([KeyCode::KeyW]);
        for _ in 0..100 {
            player
                .update(Duration::from_millis(7), &keys, (0.0, 0.0), UserCmd::default())
                .unwrap();
        }
        assert_eq!(player.state().view().command_time, 700);
        assert_eq!(player.tick_msec, 7);
    }
    #[test]
    fn menu_pause_and_spectator_switch_preserve_player_state() {
        let (mut player, spawn) = local();
        player
            .update(
                Duration::from_millis(200),
                &HashSet::from([KeyCode::KeyW]),
                (0.0, 0.0),
                UserCmd::default(),
            )
            .unwrap();
        let saved = player.state().view();
        player.pause();
        assert_eq!(saved, player.state().view());
        player.join(JoinMode::Spectator, spawn).unwrap();
        player
            .update(
                Duration::from_millis(200),
                &HashSet::from([KeyCode::Space]),
                (0.0, 0.0),
                UserCmd::default(),
            )
            .unwrap();
        assert!(player.state().view().origin[2] > saved.origin[2]);
        player.join(JoinMode::Player, spawn).unwrap();
        assert_eq!(saved, player.state().view());
    }
    #[test]
    fn tapped_attack_survives_until_the_next_fixed_pmove_tick() {
        let (mut player, _) = local();
        let before = player.entity_view().saber_move;
        let attack = UserCmd {
            buttons: jka_movement::BUTTON_ATTACK,
            ..UserCmd::default()
        };

        // At very high render FPS this frame is shorter than the 8 ms physics
        // tick, so CL_CreateCmd input must not be consumed and forgotten.
        player
            .update(Duration::from_millis(1), &HashSet::new(), (0.0, 0.0), attack)
            .unwrap();
        assert_eq!(player.entity_view().saber_move, before);

        player
            .update(
                Duration::from_millis(7),
                &HashSet::new(),
                (0.0, 0.0),
                UserCmd::default(),
            )
            .unwrap();
        assert_ne!(player.entity_view().saber_move, before);
    }

    #[test]
    fn local_saber_attack_cycle_runs_through_server_side_generic_command_path() {
        // protocol-26 playerStateFields slot 23 is fd.saberAnimLevel.
        const SABER_ANIM_LEVEL_FIELD: usize = 23;
        let (mut player, _) = local();
        assert_eq!(player.network_state().fields[SABER_ANIM_LEVEL_FIELD], 2); // SS_MEDIUM

        player
            .update(
                Duration::from_millis(8),
                &HashSet::new(),
                (0.0, 0.0),
                UserCmd {
                    generic_command: jka_movement::GENCMD_SABERATTACKCYCLE,
                    ..UserCmd::default()
                },
            )
            .unwrap();
        assert_eq!(player.network_state().fields[SABER_ANIM_LEVEL_FIELD], 3); // SS_STRONG
        // fd.saberDrawAnimLevel (slot 25) is what the HUD shows.
        assert_eq!(player.network_state().fields[25], 3);

        // OpenJK debounces repeats of the same generic command for 300 ms.
        player
            .update(
                Duration::from_millis(8),
                &HashSet::new(),
                (0.0, 0.0),
                UserCmd {
                    generic_command: jka_movement::GENCMD_SABERATTACKCYCLE,
                    ..UserCmd::default()
                },
            )
            .unwrap();
        assert_eq!(player.network_state().fields[SABER_ANIM_LEVEL_FIELD], 3);

        // Once the debounce expires, strong wraps to fast at offense level 3.
        for _ in 0..38 {
            player
                .update(
                    Duration::from_millis(8),
                    &HashSet::new(),
                    (0.0, 0.0),
                    UserCmd::default(),
                )
                .unwrap();
        }
        player
            .update(
                Duration::from_millis(8),
                &HashSet::new(),
                (0.0, 0.0),
                UserCmd {
                    generic_command: jka_movement::GENCMD_SABERATTACKCYCLE,
                    ..UserCmd::default()
                },
            )
            .unwrap();
        assert_eq!(player.network_state().fields[SABER_ANIM_LEVEL_FIELD], 1); // SS_FAST
    }

    #[test]
    fn noclip_toggles_native_pmove_type() {
        let (mut player, _) = local();
        assert!(!player.noclip);
        assert_ne!(player.view().pm_type, jka_movement::PM_NOCLIP);
        assert!(player.toggle_noclip().unwrap());
        assert!(player.noclip);
        assert_eq!(player.view().pm_type, jka_movement::PM_NOCLIP);
        assert!(!player.toggle_noclip().unwrap());
        assert_ne!(player.view().pm_type, jka_movement::PM_NOCLIP);
    }

    #[test]
    fn noclip_mouse_buttons_apply_taystjk_turbo_boost() {
        fn velocity(primary: bool, alt: bool) -> f32 {
            let (mut player, _) = local();
            player.set_noclip(true).unwrap();
            player
                .update(
                    Duration::from_millis(8),
                    &HashSet::from([KeyCode::KeyW]),
                    (0.0, 0.0),
                    UserCmd {
                        buttons: (if primary { jka_movement::BUTTON_ATTACK } else { 0 })
                            | (if alt { jka_movement::BUTTON_ALT_ATTACK } else { 0 }),
                        ..UserCmd::default()
                    },
                )
                .unwrap();
            glam::Vec3::from_array(player.view().velocity).length()
        }

        let normal = velocity(false, false);
        let primary = velocity(true, false);
        let alt = velocity(false, true);
        let both = velocity(true, true);
        assert!(primary > normal * 5.0);
        assert!(alt > normal * 5.0);
        assert!(both > primary * 5.0);
    }

    #[test]
    fn loose_maps_keep_spectator_but_cannot_join_without_collision() {
        let spawn = SpawnPoint {
            position: [0.0; 3],
            yaw: 0.0,
            ..SpawnPoint::default()
        };
        let mut player = LocalPlayer::new(None, None, spawn).unwrap();
        assert!(player.join(JoinMode::Player, spawn).is_err());
        player
            .update(
                Duration::from_millis(200),
                &HashSet::from([KeyCode::KeyW]),
                (0.0, 0.0),
                UserCmd::default(),
            )
            .unwrap();
        assert!(player.state().view().origin[0] > 0.0);
    }
}

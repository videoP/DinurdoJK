use crate::{
    camera::Camera,
    scene::{jka_position, render_position, SpawnPoint},
    surface_deformation::{LocalFootContactShim, SurfaceDeformationStamp},
};
use jka_movement::{
    angle_to_short, CollisionWorld, JoinMode, MovementPower, PlayerState, PmoveContext, TraceQuery,
    TraceResult, TraceWorld, UserCmd, BUTTON_ALT_ATTACK, BUTTON_ATTACK, BUTTON_WALKING,
    ENTITY_NONE, MAX_TICK_MSEC, MIN_TICK_MSEC, TICK_MSEC,
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

/// Render-only smoothing controls for the local player. None of these change
/// OpenJK pmove state or command generation; they only choose how fixed-step
/// samples are presented between simulation ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalPresentationSettings {
    pub smooth_player_origin: bool,
    pub smooth_third_person_origin: bool,
    pub smooth_player_animation: bool,
    pub subframe_player_angles: bool,
    pub smooth_third_person_time: bool,
}

impl Default for LocalPresentationSettings {
    fn default() -> Self {
        Self {
            smooth_player_origin: true,
            smooth_third_person_origin: true,
            smooth_player_animation: true,
            subframe_player_angles: true,
            smooth_third_person_time: true,
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
    previous_eye: [f32; 3],
    current_eye: [f32; 3],
    previous_origin: [f32; 3],
    current_origin: [f32; 3],
    previous_command_time: i32,
    current_command_time: i32,
    pending_power: Option<MovementPower>,
    // JKA cl_idrive 1 semantics: when opposite directions are both held,
    // the direction pressed most recently wins. Only one signed winner per
    // axis is needed, so this costs three bytes and needs no timestamp.
    move_priority: [i8; 3],
    noclip: bool,
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
        let spectator = PlayerState::spawn(
            jka_position(spawn.position),
            spawn.yaw.to_degrees(),
            JoinMode::Spectator,
        )?;
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
            previous_eye: eye,
            current_eye: eye,
            previous_origin: spectator_view.origin,
            current_origin: spectator_view.origin,
            previous_command_time: spectator_view.command_time,
            current_command_time: spectator_view.command_time,
            pending_power: None,
            move_priority: [0; 3],
            noclip: false,
            mouse_input: MouseInputSettings::default(),
            deformation_shim: LocalFootContactShim::new(spectator_view.origin),
        })
    }
    pub fn can_join(&self) -> bool {
        self.can_join
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
    pub fn view(&self) -> jka_movement::PlayerView {
        self.state().view()
    }
    pub fn entity_view(&self) -> jka_movement::PlayerEntityView {
        self.state().entity_view()
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
            return Err("JOIN GAME REQUIRES A COMPILED BSP AND HUMANOID ANIMATIONS".into());
        }
        if mode != self.mode {
            if self.noclip {
                if let Some(player) = &mut self.player {
                    player.set_noclip(false);
                }
                self.noclip = false;
            }
            if mode == JoinMode::Player && self.player.is_none() {
                self.player = Some(PlayerState::spawn(
                    jka_position(spawn.position),
                    spawn.yaw.to_degrees(),
                    mode,
                )?);
            } else if mode == JoinMode::Spectator {
                let view = self.state().view();
                self.spectator = PlayerState::spawn(view.origin, view.view_angles[1], mode)?;
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
        self.input_angles[0] = self.input_angles[0].clamp(-JKA_MAX_VIEW_PITCH, JKA_MAX_VIEW_PITCH);
        self.input_angles[1] = self.input_angles[1].rem_euclid(360.0);
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
        self.input_angles[0] = (self.input_angles[0] + self.mouse_input.pitch * my)
            .clamp(-JKA_MAX_VIEW_PITCH, JKA_MAX_VIEW_PITCH);
    }

    /// Convenience path used by unit tests without a timing sample. Runtime mouse
    /// input uses `apply_mouse_look_timed` so acceleration has a real timebase.
    #[cfg(test)]
    pub fn apply_mouse_look(&mut self, mouse: (f64, f64)) {
        self.apply_mouse_look_timed(mouse, Duration::from_millis(1));
    }

    pub fn update(
        &mut self,
        elapsed: Duration,
        keys: &HashSet<KeyCode>,
        mouse: (f64, f64),
        noclip_primary: bool,
        noclip_alt: bool,
    ) -> Result<(), String> {
        self.apply_mouse_look_timed(mouse, elapsed);
        self.accumulated += elapsed.min(Duration::from_millis(250));
        let tick = Duration::from_millis(self.tick_msec as u64);
        while self.accumulated >= tick {
            self.accumulated -= tick;
            let move_priority = self.move_priority;
            let state = match self.mode {
                JoinMode::Player => self.player.as_mut().expect("joined player"),
                JoinMode::Spectator => &mut self.spectator,
            };
            let walk = keys.contains(&KeyCode::ShiftLeft) || keys.contains(&KeyCode::ShiftRight);
            let magnitude = if walk { 64 } else { 127 };
            let crouch =
                keys.contains(&KeyCode::ControlLeft) || keys.contains(&KeyCode::ControlRight);
            let mut buttons = if walk { BUTTON_WALKING } else { 0 };
            if self.noclip && noclip_primary {
                buttons |= BUTTON_ATTACK;
            }
            if self.noclip && noclip_alt {
                buttons |= BUTTON_ALT_ATTACK;
            }
            let cmd = UserCmd {
                server_time: state.view().command_time + self.tick_msec,
                angles: self.input_angles.map(angle_to_short),
                buttons,
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
            state.offline_force_tick_with_msec(
                cmd.server_time,
                self.pending_power.take(),
                self.tick_msec,
            )?;
            let view = self
                .movement
                .step_with_msec(state, cmd, self.tick_msec, &mut self.world)?;
            self.current_eye = view.eye_origin();
            self.current_origin = view.origin;
            self.current_command_time = view.command_time;
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
        let yaw = self.input_angles[1] + delta_angle_degrees(view.delta_angles[1]);
        let pitch = (self.input_angles[0] + delta_angle_degrees(view.delta_angles[0]))
            .clamp(-JKA_MAX_VIEW_PITCH, JKA_MAX_VIEW_PITCH);
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
                        false,
                        false,
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
    fn mouse_pitch_uses_stock_openjk_limit() {
        let (mut player, _) = local();
        player.apply_mouse_look((0.0, -100_000.0));
        assert!((player.input_angles[0] + JKA_MAX_VIEW_PITCH).abs() < 1.0e-5);

        player.apply_mouse_look((0.0, 200_000.0));
        assert!((player.input_angles[0] - JKA_MAX_VIEW_PITCH).abs() < 1.0e-5);
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
                .update(Duration::from_millis(7), &keys, (0.0, 0.0), false, false)
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
                false,
                false,
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
                false,
                false,
            )
            .unwrap();
        assert!(player.state().view().origin[2] > saved.origin[2]);
        player.join(JoinMode::Player, spawn).unwrap();
        assert_eq!(saved, player.state().view());
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
                    primary,
                    alt,
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
        };
        let mut player = LocalPlayer::new(None, None, spawn).unwrap();
        assert!(player.join(JoinMode::Player, spawn).is_err());
        player
            .update(
                Duration::from_millis(200),
                &HashSet::from([KeyCode::KeyW]),
                (0.0, 0.0),
                false,
                false,
            )
            .unwrap();
        assert!(player.state().view().origin[0] > 0.0);
    }
}

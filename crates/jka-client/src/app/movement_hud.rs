//! Movement hud.
use crate::app::{
    playerstate_vec3, scene, strafehelper_walking_anim, ui, App, Instant, JoinMode, KeyCode,
    OverlayMode, SessionPhase, ViewLatchMode,
};

impl App {
    /// `cg_speedometer & SPEEDOMETER_ENABLE` or `cg_strafeHelper & SHELPER_ACCELMETER`.
    pub(in crate::app) fn speedometer_active(&self) -> bool {
        self.japro_cg.speedometer.flags & crate::speedometer::flag::ENABLE != 0
            || self.strafe_helper.flags & ui::SHELPER_ACCELMETER != 0
    }

    /// Origin, `pm_time` and `fd.forceJumpZStart` of the local player: the
    /// fields the speedometer reads that the movement HUD state does not carry.
    pub(in crate::app) fn speedometer_player_extras(&self) -> ([f32; 3], i32, f32) {
        if let Some(ps) = self.live_player_state() {
            return (
                playerstate_vec3(ps, "origin").unwrap_or_default(),
                ps.field_i32("pm_time").unwrap_or(0),
                ps.field_f32("fd.forceJumpZStart").unwrap_or(0.0),
            );
        }
        if let Some(player) = &self.local_server {
            let view = player.view();
            return (view.origin, view.pm_time, 0.0);
        }
        ([0.0; 3], 0, 0.0)
    }

    /// jaPRO `CG_SpeedometerSettings_f`: list the `cg_speedometer` bits, or toggle one.
    pub(in crate::app) fn speedometer_command(&mut self, args: &[&str]) {
        use crate::speedometer::{toggle, TOGGLE_LABELS};
        let flags = self.japro_cg.speedometer.flags;
        let Some(arg) = args.first() else {
            self.push_console_line("^5num  on  name".to_owned());
            for (index, label) in TOGGLE_LABELS.iter().enumerate() {
                let mark = if flags & (1 << index) != 0 { 'X' } else { ' ' };
                self.push_console_line(format!("{index:>3}  [{mark}] {label}"));
            }
            return;
        };
        let last = TOGGLE_LABELS.len() - 1;
        let index = match arg.trim().parse::<usize>() {
            Ok(index) if index <= last => index,
            _ => {
                self.push_console_line(format!(
                    "^3speedometer:^7 invalid range: {arg} [0, {last}]"
                ));
                return;
            }
        };
        let value = toggle(flags, index);
        if let Err(error) = self.set_console_cvar("cg_speedometer", &value.to_string()) {
            self.push_console_line(format!("^1{error}"));
            return;
        }
        let state = if value & (1 << index) != 0 {
            "^2Enabled"
        } else {
            "^1Disabled"
        };
        self.push_console_line(format!("{} {state}^7", TOGGLE_LABELS[index]));
    }

    /// Step the jaPRO speedometer by one rendered frame (`DF_SetSpeedometer` +
    /// `DF_DrawSpeedometer` and the readouts hanging off it).
    pub(in crate::app) fn update_speedometer(&mut self, now: Instant) {
        if !self.speedometer_active() || self.overlay == OverlayMode::HudEdit {
            // The editor shows a static preview instead; leave the live state alone.
            self.speedometer_ui = None;
            self.speedometer_last_frame = None;
            return;
        }
        let in_game = self.live_player_state().is_some()
            || self
                .local_server
                .as_ref()
                .is_some_and(|player| player.mode == JoinMode::Player);
        if !in_game {
            self.speedometer_ui = None;
            return;
        }
        let hud = self.current_movement_hud_state();
        let (origin, pm_time, force_jump_z_start) = self.speedometer_player_extras();
        let frame_ms = self.speedometer_last_frame.map_or(0, |last| {
            now.saturating_duration_since(last).as_millis().min(1000) as i32
        });
        self.speedometer_last_frame = Some(now);
        let real_ms = now
            .saturating_duration_since(self.speedometer_clock)
            .as_millis() as i64;
        let v = hud.velocity[0].hypot(hud.velocity[1]);
        let input = crate::speedometer::Input {
            v,
            vxyz: v.hypot(hud.velocity[2]),
            vertical: hud.velocity[2],
            wishspeed: crate::strafehelper::wishspeed(
                &hud,
                &self.strafe_helper,
                self.video.fps_cap,
            ),
            player_speed: hud.player_speed,
            cgaz_frametime: crate::strafehelper::frametime(&self.strafe_helper, self.video.fps_cap),
            on_ground: hud.grounded,
            pm_time,
            view_yaw: hud.view_yaw,
            origin,
            force_jump_z_start,
            time_ms: real_ms as i32,
            frame_ms,
            real_ms,
            strafehelper_accelmeter: self.strafe_helper.flags & ui::SHELPER_ACCELMETER != 0,
            lagometer: self.japro_cg.lagometer,
            width_ratio_coef: self.width_ratio_coef(),
        };
        let settings = self.japro_cg.speedometer;
        self.speedometer_ui = self.speedometer.update(&settings, &input);
    }

    /// `cgs.widthRatioCoef` for the window as it is now (`cl_ratioFix 1`, the default).
    pub(in crate::app) fn width_ratio_coef(&self) -> f32 {
        self.window.as_ref().map_or(1.0, |window| {
            let size = window.inner_size();
            crate::lagometer::width_ratio_coef(size.width, size.height)
        })
    }

    /// A `MP_INGAME` StringEd line (`CG_GetStringEdString("MP_INGAME", name)`), falling
    /// back to the English text when the table has no entry.
    pub(in crate::app) fn ingame_string(&mut self, name: &str, fallback: &str) -> String {
        // Loads the table on first use.
        let _ = self.translate_server_text(b"x");
        let text = self.stringed.as_ref().map_or(String::new(), |table| {
            crate::cgame::bytes_to_lossless_ascii(table.get(&format!("MP_INGAME_{name}")))
        });
        if text.is_empty() {
            fallback.to_owned()
        } else {
            text
        }
    }

    /// jaPRO `CG_DrawLagometer` (graph and the connection warning) for this frame.
    pub(in crate::app) fn update_lagometer(&mut self) {
        let settings = self.japro_cg.lagometer;
        let state = self
            .game_session
            .as_ref()
            .filter(|session| session.phase == SessionPhase::Playing)
            .map(|session| {
                let local_server = session.local;
                // CG_DrawDisconnect: not for the local server or a demo.
                let link = if local_server || !session.live {
                    crate::lagometer::Link::Ok
                } else {
                    // GetUserCmd(currentCmdNumber - REAL_CMD_BACKUP + 1): the oldest
                    // buffered command, which a healthy link has long since acknowledged.
                    let cmd_server_time = self.net.as_ref().map_or(0, |net| {
                        let net_session = net.session();
                        let oldest = net_session.cmd_number() - settings.real_command_backup() + 1;
                        net_session.command(oldest).map_or(0, |cmd| cmd.server_time)
                    });
                    let snapshot_command_time = session
                        .current_snapshot
                        .as_ref()
                        .and_then(|snapshot| snapshot.player_state.field_i32("commandTime"))
                        .unwrap_or(0);
                    session.lagometer.link(
                        session.map_change,
                        cmd_server_time,
                        snapshot_command_time,
                    )
                };
                let snc = self.network.no_predict || session.client_game.synchronous_clients();
                (local_server, snc, link)
            });
        let Some((local_server, snc, link)) = state else {
            self.lagometer_ui = None;
            return;
        };

        let mut input = crate::lagometer::DrawInput {
            width_ratio_coef: self.width_ratio_coef(),
            local_server,
            snc,
            link,
            ..Default::default()
        };
        match link {
            crate::lagometer::Link::Ok => {}
            crate::lagometer::Link::Interrupted => {
                input.interrupted_text =
                    self.ingame_string("CONNECTION_INTERRUPTED", "Connection Interrupted");
            }
            crate::lagometer::Link::MapChange => {
                input.map_change_text =
                    self.ingame_string("SERVER_CHANGING_MAPS", "Server Changing Maps");
                input.please_wait_text = self.ingame_string("PLEASE_WAIT", "Please wait...");
            }
        }
        self.lagometer_ui = self
            .game_session
            .as_ref()
            .and_then(|session| session.lagometer.draw(&settings, &input));
    }

    pub(in crate::app) fn current_movement_hud_state(&self) -> ui::MovementHudState {
        let movement_keys_active = self.movement_keys_hud.mode != 0;
        let strafe_helper_active =
            self.strafe_helper.flags & ui::SHELPER_STYLE_MASK != 0 || self.speedometer_active();
        if !movement_keys_active && !strafe_helper_active {
            // Keep the mailbox value completely stable when neither feature is
            // visible. In particular, do not let view yaw/velocity turn into a
            // pointless per-frame renderer update while the HUD is disabled.
            return ui::MovementHudState::default();
        }

        let mut state = ui::MovementHudState::default();
        let ps = self.live_player_state();
        let local_session = self
            .game_session
            .as_ref()
            .is_some_and(|session| session.local);
        const PMF_FOLLOW: i32 = 4096;
        const PM_SPECTATOR: i32 = 4;

        // Follow/free-spectator state belongs to the authoritative snapshot. In
        // particular, a predicted spectator state can retain stale PMF_FOLLOW
        // briefly after unfollowing. This is the same distinction used by the
        // follow HUD: free PM_SPECTATOR movement is the spectator camera, not a
        // player movement subject for the strafehelper.
        let snapshot_ps = self
            .game_session
            .as_ref()
            .filter(|session| session.live)
            .and_then(|session| session.current_snapshot.as_ref())
            .map(|snapshot| &snapshot.player_state);
        let spectator_ps = snapshot_ps.or(ps);
        let following =
            spectator_ps.is_some_and(|ps| ps.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW != 0);
        state.spectator_free_roam = spectator_ps
            .is_some_and(|ps| ps.field_i32("pm_type").unwrap_or(0) == PM_SPECTATOR && !following);
        if spectator_ps.is_none() {
            state.spectator_free_roam = self
                .local_server
                .as_ref()
                .is_some_and(|player| player.view().pm_type == PM_SPECTATOR);
        }
        // Command state: a remote game reports the provisional usercmd; every
        // other source reads the keyboard. A followed player is the important
        // exception: our local spectator usercmd says nothing about what the
        // followed client is pressing. TaystJK reconstructs that command from
        // snapshot movementDir/flags, so do the same here.
        if following {
            if let Some(ps) = ps {
                let velocity = playerstate_vec3(ps, "velocity").unwrap_or_default();
                let cmd = crate::strafehelper::spectated_cmd(
                    ps.field_i32("movementDir").unwrap_or(-1),
                    velocity,
                    ps.field_i32("pm_flags").unwrap_or(0),
                    ps.field_i32("eFlags").unwrap_or(0),
                    strafehelper_walking_anim(ps.field_i32("legsAnim").unwrap_or(0)),
                );
                state.forward_move = cmd.forward_move;
                state.right_move = cmd.right_move;
                state.up_move = cmd.up_move;
                state.buttons = cmd.buttons;
            }
        } else {
            match ps.and(self.live_provisional).filter(|_| !local_session) {
                Some(cmd) => {
                    state.forward_move = cmd.forward_move;
                    state.right_move = cmd.right_move;
                    state.up_move = cmd.up_move;
                    state.buttons = cmd.buttons;
                }
                None => {
                    let axis = |positive: KeyCode, negative: KeyCode| -> i8 {
                        match (
                            self.movement_keys.contains(&positive),
                            self.movement_keys.contains(&negative),
                        ) {
                            (true, false) => 127,
                            (false, true) => -127,
                            _ => 0,
                        }
                    };
                    state.forward_move = axis(KeyCode::KeyW, KeyCode::KeyS);
                    state.right_move = axis(KeyCode::KeyD, KeyCode::KeyA);
                    state.up_move = axis(KeyCode::Space, KeyCode::ControlLeft);
                    if self.noclip_primary_down {
                        state.buttons |= jka_movement::BUTTON_ATTACK;
                    }
                    if self.noclip_alt_down {
                        state.buttons |= jka_movement::BUTTON_ALT_ATTACK;
                    }
                    if self.movement_keys.contains(&KeyCode::ShiftLeft) {
                        state.buttons |= jka_movement::BUTTON_WALKING;
                    }
                }
            }
        }
        if !strafe_helper_active {
            // MovementKeys only consumes command/button state. Leave the
            // continuous fields at their defaults so the snapshot changes only
            // when a key or button actually changes, not every time
            // velocity/yaw moves.
            return state;
        }

        // Strafehelper inputs: the predicted playerstate, or the offline Pmove
        // host when no session is running.
        const PMF_TIME_KNOCKBACK: i32 = 64;
        const PM_JETPACK: i32 = 1;
        const EF_JETPACK_ACTIVE: i32 = 1 << 11;
        const STAT_MOVEMENTSTYLE: usize = 13;
        let subframe = local_session;
        let (angles, origin, command_time);
        if let Some(ps) = ps {
            let ps_i32 = |name: &str| ps.field_i32(name).unwrap_or(0);
            angles = if subframe {
                self.local_server
                    .as_ref()
                    .map(|server| server.subframe_view_angles())
            } else {
                self.live_view_angles()
            }
            .or_else(|| playerstate_vec3(ps, "viewangles"))
            .unwrap_or_default();
            origin = playerstate_vec3(ps, "origin").unwrap_or_default();
            command_time = ps_i32("commandTime");
            state.velocity = playerstate_vec3(ps, "velocity").unwrap_or_default();
            // `speed` is a float netfield: field_i32 would return its raw bits.
            state.player_speed = ps.field_f32("speed").unwrap_or(250.0).max(1.0);
            state.grounded = ps
                .field_i32("groundEntityNum")
                .is_some_and(|entity| entity != 1023);
            if self.active_server_mod() == crate::net::mod_support::ServerMod::Japro {
                state.move_style = ps.stats[STAT_MOVEMENTSTYLE];
            }
            state.knockback = ps_i32("pm_flags") & PMF_TIME_KNOCKBACK != 0;
            state.jetpack_pm_type = ps_i32("pm_type") == PM_JETPACK;
            state.jetpack_active = ps_i32("eFlags") & EF_JETPACK_ACTIVE != 0;
            state.in_vehicle = ps_i32("m_iVehicleNum") != 0;
        } else if let Some(player) = &self.local_server {
            let view = player.view();
            angles = player.subframe_view_angles();
            origin = view.origin;
            command_time = view.command_time;
            state.velocity = view.velocity;
            state.grounded = view.ground_entity != 1023;
            state.knockback = view.pm_flags & PMF_TIME_KNOCKBACK != 0;
            state.jetpack_pm_type = view.pm_type == PM_JETPACK;
        } else {
            angles = [
                -self.camera.pitch.to_degrees(),
                self.camera.yaw.to_degrees(),
                0.0,
            ];
            origin = [0.0; 3];
            command_time = 0;
        }
        state.origin = origin;
        state.view_pitch = angles[0];
        state.view_yaw = angles[1];
        state.view_roll = angles[2];
        state.fov_x = self.japro_effective_fov(Instant::now());

        let mut ground = self.strafe_ground.get();
        state.was_grounded = ground.update(command_time, state.grounded);
        self.strafe_ground.set(ground);

        // Lines are anchored at the player origin in third person (and for
        // SHELPER_ORIGINAL), so give the projection the eye-to-origin offset.
        state.third_person = matches!(self.render_view_latch, ViewLatchMode::ThirdPerson(_));
        let mut eye = self.camera.position;
        if let Some(session) = self.game_session.as_ref() {
            eye.y += session.view_offset_z;
        }
        let eye = scene::jka_position(eye.to_array());
        state.eye_to_origin = std::array::from_fn(|i| origin[i] - eye[i]);
        state
    }
}

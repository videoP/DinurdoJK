//! Connection mod preferences.
use crate::app::{App, ElementState, Instant, KeyCode, KeyEvent, OverlayMode, SocketAddr};

impl App {
    pub(in crate::app) fn refresh_japro_saved_logins(&mut self) {
        match crate::credential_store::list() {
            Ok(entries) => {
                self.japro_saved_logins = entries;
                self.japro_credential_error = None;
            }
            Err(error) => self.japro_credential_error = Some(error),
        }
    }

    pub(in crate::app) fn current_japro_server_endpoint(&self) -> Option<SocketAddr> {
        (self.live_connected()
            && self.connected_server_mod() == crate::net::mod_support::ServerMod::Japro)
            .then(|| self.net.as_ref().map(|net| net.session().server()))
            .flatten()
    }

    pub(in crate::app) fn save_japro_login_for_current_server(&mut self) -> Result<(), String> {
        let server = self
            .current_japro_server_endpoint()
            .ok_or_else(|| "Join a jaPRO server before saving an automatic login".to_owned())?;
        if self.network.ui_username.is_empty() || self.network.ui_password.is_empty() {
            return Err("Enter both jaPRO username and password first".to_owned());
        }
        crate::credential_store::save(
            server,
            &self.network.ui_username,
            &self.network.ui_password,
        )?;
        self.refresh_japro_saved_logins();
        Ok(())
    }

    pub(in crate::app) fn forget_japro_login(&mut self, server: SocketAddr) -> Result<(), String> {
        crate::credential_store::delete(server)?;
        self.refresh_japro_saved_logins();
        Ok(())
    }

    /// Auto-login only after the gamestate identifies jaPRO, and only with a
    /// credential whose key exactly equals the resolved UDP endpoint. Server
    /// names / hostnames are deliberately not accepted as credential identity.
    pub(in crate::app) fn try_japro_autologin(&mut self) {
        let Some(server) = self.current_japro_server_endpoint() else {
            return;
        };
        if self.japro_autologin_attempted == Some(server) {
            return;
        }
        self.japro_autologin_attempted = Some(server);
        match crate::credential_store::read(server) {
            Ok(Some(credential)) => {
                let valid = !credential.username.is_empty()
                    && !credential.password.is_empty()
                    && !credential.username.chars().any(char::is_whitespace)
                    && !credential.password.chars().any(char::is_whitespace)
                    && !credential
                        .username
                        .chars()
                        .any(|c| matches!(c, ';' | '"' | '\\'))
                    && !credential
                        .password
                        .chars()
                        .any(|c| matches!(c, ';' | '"' | '\\'));
                if !valid {
                    self.push_console_line(format!(
                        "^3Auto-login skipped:^7 saved credential for {server} is invalid"
                    ));
                    return;
                }
                self.network.ui_username = credential.username.clone();
                // Never copy the saved password into ui_password or config. Build
                // the one reliable command directly, then let the local secret drop.
                self.forward_command_to_server(&format!(
                    "login {} {}",
                    credential.username, credential.password
                ));
                self.push_console_line(format!(
                    "^2jaPRO auto-login:^7 {} @ {server}",
                    credential.username
                ));
            }
            Ok(None) => {}
            Err(error) => {
                self.japro_credential_error = Some(error.clone());
                self.push_console_line(format!("^3jaPRO auto-login unavailable:^7 {error}"));
            }
        }
    }

    /// The connected server's mod, from its serverinfo configstring.
    pub(in crate::app) fn connected_server_mod(&self) -> crate::net::mod_support::ServerMod {
        self.net
            .as_ref()
            .map(|net| {
                crate::net::mod_support::ServerMod::detect(crate::net::mod_support::server_info(
                    &net.session().decoder().configstrings,
                ))
            })
            .unwrap_or(crate::net::mod_support::ServerMod::Unknown)
    }

    /// The mod whose client-side settings the UI offers: the connected server's,
    /// or jaPRO for a local game running on `fs_game japro`. `None` when neither
    /// exists. Gameplay behaviour that depends on a real server (userinfo, jump
    /// height shading, flipkick) keeps using [`Self::active_server_mod`].
    pub(in crate::app) fn ui_mod(&self) -> Option<crate::net::mod_support::ServerMod> {
        use crate::net::mod_support::ServerMod;
        if self.net.is_some() {
            return Some(self.connected_server_mod());
        }
        let local = self
            .game_session
            .as_ref()
            .is_some_and(|session| session.local);
        let japro_dir = self
            .game
            .as_ref()
            .and_then(|dir| dir.file_name())
            .is_some_and(|name| name.eq_ignore_ascii_case("japro"));
        (local && japro_dir).then_some(ServerMod::Japro)
    }

    /// jaPRO's cgame republishes `cg_displayCameraPosition` and
    /// `cg_displayNetSettings` (CVAR_ROM|CVAR_USERINFO) whenever their inputs
    /// change. Recompute them and report whether a jaPRO server needs new userinfo.
    pub(in crate::app) fn refresh_japro_userinfo_state(&mut self) -> bool {
        let camera = format!(
            "{} {} {}",
            i32::from(self.third_person.enabled),
            self.third_person.range as i32,
            self.third_person.vert_offset as i32,
        );
        let net = format!(
            "{} {} {}",
            self.network.max_packets,
            self.network.time_nudge,
            self.effective_fps_cap(),
        );
        let changed = camera != self.network.display_camera_position
            || net != self.network.display_net_settings;
        self.network.display_camera_position = camera;
        self.network.display_net_settings = net;
        changed
            && self.live_connected()
            && self.connected_server_mod() == crate::net::mod_support::ServerMod::Japro
    }

    /// CL_CheckUserinfo after a CVAR_USERINFO change, or a staged change while
    /// the Profile page is editing (sent by its APPLY button).
    pub(in crate::app) fn request_userinfo_send(&mut self) {
        if self.profile_defer_send {
            self.profile_userinfo_pending = true;
        } else if self.console_buffer_running {
            self.console_userinfo_dirty = true;
        } else {
            self.send_userinfo();
        }
    }

    /// Shared live playerState. Remote play prefers client prediction; the
    /// in-process server is already authoritative and reads its current snapshot.
    pub(in crate::app) fn live_player_state(&self) -> Option<&jka_protocol::server::PlayerState> {
        let session = self.game_session.as_ref().filter(|session| session.live)?;
        if session.local {
            return session
                .current_snapshot
                .as_ref()
                .map(|snapshot| &snapshot.player_state);
        }
        self.predictor.predicted().or_else(|| {
            session
                .current_snapshot
                .as_ref()
                .map(|snapshot| &snapshot.player_state)
        })
    }

    /// True only for a joined remote game whose current predicted/display Pmove
    /// step says OpenJK owns the local view angle. This is presentation metadata;
    /// it must never change the ordinary/non-subframe camera path.
    pub(in crate::app) fn live_remote_view_forced(&self) -> bool {
        self.game_session
            .as_ref()
            .is_some_and(|session| session.live && !session.local)
            && self.predictor.predicted().is_some()
            && self.predictor.view_forced()
    }

    /// View angles for this render frame: the predicted playerstate's angles
    /// (PM_UpdateViewAngles for the newest usercmd) advanced by whatever mouse
    /// motion cl.viewangles has accumulated since that command was built. The
    /// camera then turns at frame rate while usercmds stay paced by
    /// cl_commandRate. Not applied when following, dead or when prediction is
    /// off, where the view belongs to the server.
    pub(in crate::app) fn live_view_angles(&self) -> Option<[f32; 3]> {
        const PMF_FOLLOW: i32 = 4096;
        const PM_DEAD: i32 = 5;
        if self.network.no_predict {
            return self.live_view_angles_unpredicted();
        }
        let net = self.net.as_ref()?;
        let ps = self.predictor.predicted()?;
        let pm_type = ps.field_i32("pm_type").unwrap_or(0);
        if ps.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW != 0 || pm_type >= PM_DEAD {
            return None;
        }
        let _ = net;
        let predicted_with = self.predictor.predicted_command_angles()?;
        let short_delta = |axis: usize| {
            let now = jka_movement::angle_to_short(self.live_input.view_angles[axis]);
            (now.wrapping_sub(predicted_with[axis]) as i16) as f32 * (360.0 / 65536.0)
        };
        let pitch = (ps.field_f32("viewangles[0]")? + short_delta(0))
            .clamp(-16000.0 * 360.0 / 65536.0, 16000.0 * 360.0 / 65536.0);
        Some([
            pitch,
            ps.field_f32("viewangles[1]")? + short_delta(1),
            ps.field_f32("viewangles[2]")?,
        ])
    }

    /// `cg_noPredict 1`: CG_InterpolatePlayerState(qtrue) keeps the origin and velocity from the
    /// snapshots but "grabs the latest angles": PM_UpdateViewAngles(out, newest usercmd), i.e. the
    /// command's angles plus the snapshot's delta_angles. Without this the camera turns with the
    /// snapshot's angles, a round trip plus interpolation behind the mouse. Here the newest
    /// command can be a cl_commandRate interval old, so cl.viewangles is used directly, like the
    /// predicted path's `short_delta`.
    pub(in crate::app) fn live_view_angles_unpredicted(&self) -> Option<[f32; 3]> {
        const PMF_FOLLOW: i32 = 4096;
        const PM_DEAD: i32 = 5;
        let session = self
            .game_session
            .as_ref()
            .filter(|session| session.live && !session.local)?;
        let ps = &session.current_snapshot.as_ref()?.player_state;
        if ps.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW != 0
            || ps.field_i32("pm_type").unwrap_or(0) >= PM_DEAD
        {
            return None;
        }
        let axis_angle = |axis: usize| {
            let command = jka_movement::angle_to_short(self.live_input.view_angles[axis]);
            let delta = ps.field_i32(&format!("delta_angles[{axis}]")).unwrap_or(0);
            // PM_UpdateViewAngles: temp = cmd->angles[i] + ps->delta_angles[i], in 16-bit angle units.
            i32::from(command.wrapping_add(delta) as i16)
        };
        // Don't let the player look up or down more than 90 degrees.
        let pitch = axis_angle(0).clamp(-16000, 16000);
        let to_degrees = |short: i32| short as f32 * (360.0 / 65536.0);
        Some([
            to_degrees(pitch),
            to_degrees(axis_angle(1)),
            ps.field_f32("viewangles[2]")?,
        ])
    }

    pub(in crate::app) fn active_server_mod(&self) -> crate::net::mod_support::ServerMod {
        self.net
            .as_ref()
            .map(|net| {
                crate::net::mod_support::ServerMod::detect(crate::net::mod_support::server_info(
                    &net.session().decoder().configstrings,
                ))
            })
            .unwrap_or(crate::net::mod_support::ServerMod::Unknown)
    }

    pub(in crate::app) fn japro_flipkick_restricted(&self) -> bool {
        const RESTRICT_FLIPKICKBIND: i32 = 1 << 7;
        let Some(net) = self.net.as_ref() else {
            return false;
        };
        let serverinfo =
            crate::net::mod_support::server_info(&net.session().decoder().configstrings);
        jka_protocol::commands::info_value(serverinfo, b"restricts")
            .map_or(0, jka_protocol::commands::atoi)
            & RESTRICT_FLIPKICKBIND
            != 0
    }

    /// TaystJK CG_Flipkick_f: restrictions are server-authored and the command
    /// itself only arms the asynchronous CGame frame sequence.
    pub(in crate::app) fn start_japro_flipkick(&mut self) {
        if self.japro_flipkick_restricted() {
            return;
        }
        self.japro_flipkick_frames = 1;
    }

    /// Port of TaystJK CG_DoAsync's flipkick block.  This deliberately counts
    /// client frames rather than milliseconds because that is what the source
    /// implementation and its cg_fk* cvars do.
    pub(in crate::app) fn tick_japro_flipkick(&mut self) {
        if self.japro_flipkick_restricted() {
            return;
        }
        if self.japro_flipkick_frames > self.japro_fk_duration {
            self.japro_flipkick_moveup = false;
            self.japro_flipkick_frames = 0;
            self.japro_flipkick_jumps = 0;
        } else if self.japro_flipkick_frames != 0 {
            if self.japro_flipkick_jumps == 1 {
                if self.japro_flipkick_frames > self.japro_fk_first_jump_duration {
                    self.japro_flipkick_moveup = false;
                    self.japro_flipkick_jumps += 1;
                }
            } else if self.japro_flipkick_jumps == 2 {
                if self.japro_flipkick_frames > self.japro_fk_second_jump_delay {
                    self.japro_flipkick_moveup = true;
                    self.japro_flipkick_jumps += 1;
                }
            } else if self.japro_flipkick_frames % 2 != 0 {
                self.japro_flipkick_moveup = true;
                self.japro_flipkick_jumps += 1;
            } else {
                self.japro_flipkick_moveup = false;
                self.japro_flipkick_jumps += 1;
            }
            self.japro_flipkick_frames += 1;
        }
    }

    pub(in crate::app) fn japro_zoom_fov_integer(&self) -> i32 {
        // vmCvar_t.integer truncates the cg_zoomFov string to an integer in the
        // TaystJK code paths below. Rust's cast has the same truncation toward 0.
        self.japro_zoom_fov as i32
    }

    /// Port of TaystJK CG_CalcFov's held +zoom interpolation. TaystJK uses
    /// ZOOM_OUT_TIME (100ms) for both entering and leaving this general zoom.
    pub(in crate::app) fn japro_effective_fov(&self, now: Instant) -> f32 {
        const ZOOM_OUT_TIME_MS: f32 = 100.0;
        let base = self.camera.cg_fov();
        let Some(transition_at) = self.japro_zoom_transition_at else {
            return base;
        };
        let f =
            now.saturating_duration_since(transition_at).as_secs_f32() * 1000.0 / ZOOM_OUT_TIME_MS;
        let zoom = self.japro_zoom_fov_integer();

        if self.live_buttons.contains("+zoom") {
            if f > 1.0 {
                if zoom < 1 {
                    1.0
                } else if zoom > 176 {
                    176.0
                } else {
                    zoom as f32
                }
            } else if zoom < 1 {
                base + f * (1.0 - base)
            } else if zoom > 178 {
                base + f * (178.0 - base)
            } else {
                base + f * (zoom as f32 - base)
            }
        } else if f <= 1.0 {
            if zoom < 1 {
                zoom as f32 + f * (base - 1.0)
            } else if zoom > 176 {
                zoom as f32 + f * (base - 176.0)
            } else {
                zoom as f32 + f * (base - zoom as f32)
            }
        } else {
            base
        }
    }

    /// TaystJK UIMENU_VOICECHAT: jaPRO + ui_vgs opens ingame_vgs. DinurdoJK
    /// does not substitute a made-up stock Raven voice menu for the other path.
    pub(in crate::app) fn open_vgs_voicechat(&mut self) {
        if !self.live_connected()
            || self.ui_vgs == 0
            || self.active_server_mod() != crate::net::mod_support::ServerMod::Japro
        {
            return;
        }
        self.vgs_menu = crate::vgs::Menu::Main;
        self.set_overlay(OverlayMode::Vgs);
    }

    pub(in crate::app) fn activate_vgs_action(&mut self, action: crate::vgs::Action) {
        match action {
            crate::vgs::Action::Menu(menu) => {
                self.vgs_menu = menu;
                self.egui_repaint_requested = true;
            }
            crate::vgs::Action::Command(token) => {
                self.forward_command_to_server(&format!("vgs_cmd {token}"));
                self.vgs_menu = crate::vgs::Menu::Main;
                self.set_overlay(OverlayMode::None);
            }
        }
    }

    pub(in crate::app) fn handle_vgs_key(&mut self, event: &KeyEvent, code: KeyCode) {
        if event.state != ElementState::Pressed || event.repeat {
            return;
        }
        if code == KeyCode::Escape {
            // TaystJK ingame_vgs onESC closes all; it does not navigate back.
            self.vgs_menu = crate::vgs::Menu::Main;
            self.set_overlay(OverlayMode::None);
            return;
        }
        let key = match code {
            KeyCode::KeyA => 'a',
            KeyCode::KeyB => 'b',
            KeyCode::KeyC => 'c',
            KeyCode::KeyD => 'd',
            KeyCode::KeyE => 'e',
            KeyCode::KeyF => 'f',
            KeyCode::KeyG => 'g',
            KeyCode::KeyH => 'h',
            KeyCode::KeyI => 'i',
            KeyCode::KeyJ => 'j',
            KeyCode::KeyK => 'k',
            KeyCode::KeyL => 'l',
            KeyCode::KeyM => 'm',
            KeyCode::KeyN => 'n',
            KeyCode::KeyO => 'o',
            KeyCode::KeyP => 'p',
            KeyCode::KeyQ => 'q',
            KeyCode::KeyR => 'r',
            KeyCode::KeyS => 's',
            KeyCode::KeyT => 't',
            KeyCode::KeyU => 'u',
            KeyCode::KeyV => 'v',
            KeyCode::KeyW => 'w',
            KeyCode::KeyX => 'x',
            KeyCode::KeyY => 'y',
            KeyCode::KeyZ => 'z',
            _ => return,
        };
        if let Some(action) = crate::vgs::action_for_key(self.vgs_menu, key) {
            self.activate_vgs_action(action);
        }
    }
}

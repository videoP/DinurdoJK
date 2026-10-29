//! Connection mod preferences.
use crate::app::{App, ElementState, Instant, KeyCode, KeyEvent, OverlayMode, SocketAddr};

impl App {
    pub(in crate::app) fn refresh_saved_accounts(&mut self) {
        match crate::credential_store::list() {
            Ok(entries) => {
                self.saved_accounts = entries;
                self.account_credential_error = None;
            }
            Err(error) => self.account_credential_error = Some(error),
        }
    }

    pub(in crate::app) fn current_account_server_endpoint(&self) -> Option<SocketAddr> {
        self.live_connected()
            .then(|| self.net.as_ref().map(|net| net.session().server()))
            .flatten()
    }

    fn current_account_server_name(&self) -> String {
        self.current_server_hostname()
            .map(|name| crate::logging::strip_jka_colors(&name).trim().to_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_default()
            .chars()
            .take(96)
            .collect()
    }

    pub(in crate::app) fn begin_new_account(&mut self, server: SocketAddr) {
        use crate::credential_store::AccountType;

        let account_type = AccountType::ALL
            .iter()
            .copied()
            .find(|kind| {
                !self
                    .saved_accounts
                    .iter()
                    .any(|entry| entry.server == server && entry.account_type == *kind)
            })
            .unwrap_or(AccountType::Login);
        self.account_editor = Some(crate::app::AccountEditor {
            server,
            server_name: self.current_account_server_name(),
            account_type,
            original_type: None,
            username: String::new(),
            password: String::new(),
        });
        self.account_credential_error = None;
    }

    pub(in crate::app) fn begin_edit_account(
        &mut self,
        server: SocketAddr,
        account_type: crate::credential_store::AccountType,
    ) {
        let Some(saved) = self
            .saved_accounts
            .iter()
            .find(|entry| entry.server == server && entry.account_type == account_type)
            .cloned()
        else {
            self.account_credential_error = Some(format!(
                "No saved {} account for {server}",
                account_type.label()
            ));
            return;
        };
        self.account_editor = Some(crate::app::AccountEditor {
            server,
            server_name: saved.server_name.unwrap_or_default(),
            account_type: saved.account_type,
            original_type: Some(saved.account_type),
            username: saved.username,
            // Do not pull the existing password out of Credential Manager just
            // to render an editor. Blank means preserve it when saving.
            password: String::new(),
        });
        self.account_credential_error = None;
    }

    pub(in crate::app) fn save_account_edit(
        &mut self,
        server: SocketAddr,
        server_name: &str,
        account_type: crate::credential_store::AccountType,
        original_type: Option<crate::credential_store::AccountType>,
        username: &str,
        password: &str,
    ) -> Result<(), String> {
        use crate::credential_store::AccountType;

        const USERNAME_LIMIT: usize = 64;
        const PASSWORD_LIMIT: usize = 128;
        const SERVER_NAME_LIMIT: usize = 96;

        let server_name = server_name.trim();
        let username = username.trim();
        if server_name.chars().count() > SERVER_NAME_LIMIT {
            return Err(format!("Server names are limited to {SERVER_NAME_LIMIT} characters"));
        }
        if server_name.chars().any(char::is_control) {
            return Err("Server names cannot contain control characters".to_owned());
        }

        let invalid_command_arg = |value: &str| {
            value.chars().any(char::is_whitespace)
                || value.chars().any(|c| matches!(c, ';' | '"' | '\\'))
        };
        if account_type == AccountType::Login {
            if username.is_empty() {
                return Err("Enter a username for a Login account".to_owned());
            }
            if username.chars().count() > USERNAME_LIMIT {
                return Err(format!("Usernames are limited to {USERNAME_LIMIT} characters"));
            }
            if invalid_command_arg(username) {
                return Err(
                    "Login usernames cannot contain spaces, semicolons, quotes, or backslashes"
                        .to_owned(),
                );
            }
        }

        if let Some(original_type) = original_type {
            if original_type != account_type
                && crate::credential_store::read(server, account_type)?.is_some()
            {
                return Err(format!(
                    "A saved {} account already exists for {server}",
                    account_type.label()
                ));
            }
        }

        let stored_password = if password.is_empty() {
            original_type
                .map(|kind| {
                    crate::credential_store::read(server, kind)?.ok_or_else(|| {
                        format!("No saved {} account for {server}", kind.label())
                    })
                })
                .transpose()?
                .map(|credential| credential.password)
        } else {
            None
        };
        let password = stored_password.as_deref().unwrap_or(password);
        if password.is_empty() {
            return Err("Enter a password".to_owned());
        }
        if password.chars().count() > PASSWORD_LIMIT {
            return Err(format!("Passwords are limited to {PASSWORD_LIMIT} characters"));
        }
        if password.chars().any(char::is_control) {
            return Err("Passwords cannot contain control characters".to_owned());
        }
        if matches!(account_type, AccountType::Login | AccountType::Admin)
            && invalid_command_arg(password)
        {
            return Err(format!(
                "{} passwords cannot contain spaces, semicolons, quotes, or backslashes",
                account_type.label()
            ));
        }

        let stored_username = if account_type == AccountType::Login {
            username
        } else {
            ""
        };
        crate::credential_store::save(
            server,
            account_type,
            (!server_name.is_empty()).then_some(server_name),
            stored_username,
            password,
        )?;
        if let Some(original_type) = original_type {
            if original_type != account_type {
                crate::credential_store::delete(server, original_type)?;
            }
        }
        self.refresh_saved_accounts();
        Ok(())
    }

    fn validate_saved_command_arg(value: &str) -> bool {
        !value.is_empty()
            && !value.chars().any(char::is_whitespace)
            && !value.chars().any(|c| matches!(c, ';' | '"' | '\\'))
    }

    pub(in crate::app) fn login_with_saved_account(
        &mut self,
        server: SocketAddr,
        account_type: crate::credential_store::AccountType,
    ) -> Result<(), String> {
        use crate::credential_store::AccountType;

        let current = self
            .current_account_server_endpoint()
            .ok_or_else(|| "Join a server before using a saved account".to_owned())?;
        if current != server {
            return Err(format!(
                "Saved account belongs to {server}, but the current server is {current}"
            ));
        }
        let credential = crate::credential_store::read(server, account_type)?.ok_or_else(|| {
            format!("No saved {} account for {server}", account_type.label())
        })?;

        match account_type {
            AccountType::Login => {
                if !Self::validate_saved_command_arg(&credential.username)
                    || !Self::validate_saved_command_arg(&credential.password)
                {
                    return Err(format!("Saved Login account for {server} is invalid"));
                }
                self.forward_command_to_server(&format!(
                    "login {} {}",
                    credential.username, credential.password
                ));
                self.console_status = format!("LOGIN SENT FOR {}", credential.username);
                self.push_console_line(format!(
                    "^2Saved login:^7 {} @ {server}",
                    credential.username
                ));
            }
            AccountType::Admin => {
                if !Self::validate_saved_command_arg(&credential.password) {
                    return Err(format!("Saved Admin account for {server} is invalid"));
                }
                self.forward_command_to_server(&format!("amlogin {}", credential.password));
                self.console_status = "ADMIN LOGIN SENT".to_owned();
                self.push_console_line(format!("^2Saved admin login sent:^7 {server}"));
            }
            AccountType::Rcon => {
                if credential.password.is_empty() || credential.password.chars().any(char::is_control)
                {
                    return Err(format!("Saved RCON account for {server} is invalid"));
                }
                self.set_console_cvar("rconPassword", &credential.password)?;
                self.console_status = "RCON PASSWORD SET".to_owned();
                self.push_console_line(format!("^2Saved RCON password loaded:^7 {server}"));
            }
            AccountType::Server => {
                if credential.password.is_empty() || credential.password.chars().any(char::is_control)
                {
                    return Err(format!("Saved Server account for {server} is invalid"));
                }
                self.network.password = credential.password;
                self.console_status = "SERVER PASSWORD LOADED".to_owned();
                self.push_console_line(format!("^2Saved server password loaded:^7 {server}"));
            }
        }
        Ok(())
    }

    pub(in crate::app) fn logout_saved_account(
        &mut self,
        server: SocketAddr,
        account_type: crate::credential_store::AccountType,
    ) -> Result<(), String> {
        use crate::credential_store::AccountType;

        let current = self
            .current_account_server_endpoint()
            .ok_or_else(|| "Join a server before logging out".to_owned())?;
        if current != server {
            return Err(format!(
                "Saved account belongs to {server}, but the current server is {current}"
            ));
        }
        match account_type {
            AccountType::Login => {
                self.forward_command_to_server("logout");
                self.console_status = "LOGOUT SENT".to_owned();
            }
            AccountType::Admin => {
                self.forward_command_to_server("amlogout");
                self.console_status = "ADMIN LOGOUT SENT".to_owned();
            }
            AccountType::Rcon => {
                self.set_console_cvar("rconPassword", "")?;
                self.console_status = "RCON PASSWORD CLEARED".to_owned();
            }
            AccountType::Server => {
                self.network.password.clear();
                self.console_status = "SERVER PASSWORD CLEARED".to_owned();
            }
        }
        Ok(())
    }

    pub(in crate::app) fn forget_saved_account(
        &mut self,
        server: SocketAddr,
        account_type: crate::credential_store::AccountType,
    ) -> Result<(), String> {
        crate::credential_store::delete(server, account_type)?;
        if self.account_editor.as_ref().is_some_and(|editor| {
            editor.server == server && editor.original_type == Some(account_type)
        }) {
            self.account_editor = None;
        }
        self.refresh_saved_accounts();
        Ok(())
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

    /// Mark the inputs mirrored by jaPRO's ROM/userinfo camera cvar dirty.
    /// TaystJK does not publish this immediately; CG_UpdateNetUserinfo checks
    /// the modification counts only after its shared 8 second debounce expires.
    pub(in crate::app) fn mark_japro_camera_userinfo_dirty(&mut self) {
        self.japro_camera_userinfo_dirty = true;
    }

    /// Mark the inputs mirrored by jaPRO's cg_displayNetSettings dirty.
    pub(in crate::app) fn mark_japro_net_userinfo_dirty(&mut self) {
        self.japro_net_userinfo_dirty = true;
    }

    /// Port of TaystJK/jaPRO CG_UpdateNetUserinfo's shared 8000 ms debounce.
    /// Changes made inside the window collapse to the latest camera/net tuple.
    /// Crucially, queue_userinfo() does not recompute these mirrors: an unrelated
    /// userinfo send during the window must keep the last published tuple.
    pub(in crate::app) fn tick_japro_userinfo_updates(&mut self) {
        const USERINFO_DEBOUNCE_MS: i32 = 8000;

        if !self.live_connected()
            || self.connected_server_mod() != crate::net::mod_support::ServerMod::Japro
            || (!self.japro_camera_userinfo_dirty && !self.japro_net_userinfo_dirty)
            || self.profile_userinfo_pending
        {
            return;
        }
        if !self.game_session.as_ref().is_some_and(|session| session.live) {
            return;
        }
        // TaystJK's cg.time is the CGame frame's serverTime, not the timestamp
        // of the current snapshot. cl.serverTime is the matching live clock here.
        let Some(cg_time) = self.net.as_ref().map(|net| net.session().server_time()) else {
            return;
        };
        if cg_time <= self.japro_userinfo_update_debounce {
            return;
        }

        let mut changed = false;
        if self.japro_camera_userinfo_dirty {
            // Dinurdo mode 2 is spectator-only orbit. jaPRO's advertised camera
            // bit remains the stock/Tayst 0/1 third-person setting.
            let camera = format!(
                "{} {} {}",
                i32::from(self.third_person.enabled),
                self.third_person.range as i32,
                self.third_person.vert_offset as i32,
            );
            changed |= camera != self.network.display_camera_position;
            self.network.display_camera_position = camera;
            self.japro_camera_userinfo_dirty = false;
        }
        if self.japro_net_userinfo_dirty {
            let net = format!(
                "{} {} {}",
                self.network.max_packets,
                self.network.time_nudge,
                self.effective_fps_cap(),
            );
            changed |= net != self.network.display_net_settings;
            self.network.display_net_settings = net;
            self.japro_net_userinfo_dirty = false;
        }

        self.japro_userinfo_update_debounce = cg_time.saturating_add(USERINFO_DEBOUNCE_MS);
        if changed {
            if let Err(error) = self.queue_userinfo() {
                self.push_console_line(format!("^1{error}"));
            }
        }
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

    /// Canonical state for this frame's local or recorded/followed player.
    /// Alternate demo POVs have only EntityState, so keep their reconstruction
    /// separate. Snapshot fallback is for initialization before the first frame.
    pub(in crate::app) fn current_player_state(
        &self,
    ) -> Option<&jka_protocol::server::PlayerState> {
        let session = self.game_session.as_ref()?;
        let ps = session.presented_player_state().or_else(|| {
            session
                .current_snapshot
                .as_ref()
                .map(|snapshot| &snapshot.player_state)
        })?;
        if !session.live {
            match self.demo_view_mode {
                crate::app::DemoViewMode::Free => return None,
                crate::app::DemoViewMode::Follow(client)
                    if ps.field_i32("clientNum") != Some(client) =>
                {
                    return None
                }
                _ => {}
            }
        }
        Some(ps)
    }

    /// Live-only gateway retained for gameplay callers; presentation reads the
    /// session's cached frame, while Predictor still owns committed Pmove state.
    pub(in crate::app) fn live_player_state(&self) -> Option<&jka_protocol::server::PlayerState> {
        self.game_session.as_ref().filter(|session| session.live)?;
        self.current_player_state()
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
        let snapshot_ps = &self
            .game_session
            .as_ref()?
            .current_snapshot
            .as_ref()?
            .player_state;
        if snapshot_ps.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW != 0 {
            return None;
        }
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

    fn cgame_server_info(&self) -> &[u8] {
        if let Some(net) = self.net.as_ref() {
            return crate::net::mod_support::server_info(&net.session().decoder().configstrings);
        }
        self.game_session
            .as_ref()
            .map(|session| crate::net::mod_support::server_info(&session.decoder.configstrings))
            .unwrap_or_default()
    }

    fn cgame_server_mod(&self) -> crate::net::mod_support::ServerMod {
        crate::net::mod_support::ServerMod::detect(self.cgame_server_info())
    }

    pub(in crate::app) fn server_restrictions(&self) -> crate::app::restrictions::ServerRestrictions {
        crate::app::restrictions::ServerRestrictions::from_server_info(self.cgame_server_info())
    }

    pub(in crate::app) fn japro_do_restricted(&self) -> bool {
        self.server_restrictions().contains(crate::app::restrictions::DO)
    }

    /// TaystJK CG_DrawActiveFrame: enforce RESTRICT_ANGLE after the latest
    /// serverinfo/snapshot was processed, before constructing the render camera.
    pub(in crate::app) fn enforce_server_third_person_angle(&mut self, velocity: [f32; 3]) {
        let restrictions = self.server_restrictions();
        if !restrictions.contains(crate::app::restrictions::THIRD_PERSON_ANGLE) {
            return;
        }
        let japro = self.cgame_server_mod() == crate::net::mod_support::ServerMod::Japro;
        let Some(ps) = self.current_player_state() else {
            return;
        };
        const PMF_FOLLOW: i32 = 4096;
        const PERS_TEAM: usize = 3;
        const TEAM_SPECTATOR: i32 = 3;
        // MV_BOTCPM = 14 in bundled jaPRO bg_public.h (with _SPPHYSICS).
        const MV_BOTCPM: i32 = 14;
        let spectator_or_following = ps.persistant[PERS_TEAM] == TEAM_SPECTATOR
            || ps.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW != 0;
        let moving_horizontally = velocity[0].is_finite()
            && velocity[1].is_finite()
            && (velocity[0] != 0.0 || velocity[1] != 0.0);
        if restrictions.restricts_race_angle(
            japro,
            ps.stats[crate::japro_cg::STAT_RACEMODE] != 0,
            spectator_or_following,
            moving_horizontally,
            ps.stats[crate::japro_cg::STAT_MOVEMENTSTYLE] == MV_BOTCPM,
        ) && self.third_person.angle != 0.0 {
            // TaystJK writes cg_thirdPersonAngle=0; the change remains even
            // after stopping. Keep the menu/console/config synchronized.
            self.third_person.angle = 0.0;
            self.mark_config_dirty();
        }
    }

    pub(in crate::app) fn japro_do_race_restricted(&self) -> bool {
        self.cgame_server_mod() == crate::net::mod_support::ServerMod::Japro
            && self
                .current_player_state()
                .is_some_and(|ps| ps.stats[crate::japro_cg::STAT_RACEMODE] != 0)
    }

    /// Port of jaPRO CG_Do_f: one pending `vstr`, replaced by a newer `/do`,
    /// with its deadline expressed in the CGame's millisecond clock.
    pub(in crate::app) fn start_japro_do(&mut self, vstr: String, delay_ms: i32) {
        if self.japro_do_restricted() || self.japro_do_race_restricted() {
            return;
        }
        let Some(session) = self.game_session.as_mut() else {
            return;
        };
        let deadline = session.cgame_time.saturating_add(delay_ms.max(0));
        session.japro_do = Some((vstr, deadline));
    }

    /// Port of jaPRO CG_DoAsync's doVstr block. It intentionally uses `>`
    /// rather than `>=`, so a zero-delay `/do` runs on a later CGame frame.
    pub(in crate::app) fn tick_japro_do(&mut self) {
        let due_vstr = {
            let Some(session) = self.game_session.as_mut() else {
                return;
            };
            let Some((_, deadline)) = session.japro_do.as_ref() else {
                return;
            };
            if session.cgame_time <= *deadline {
                return;
            }
            session.japro_do.take().map(|(vstr, _)| vstr)
        };
        if let Some(vstr) = due_vstr {
            // trap->SendConsoleCommand queues this back to the engine. The
            // command buffer already ran this App tick, so append now and let
            // the next tick execute it rather than calling vstr recursively.
            let _ = self.append_console_commands(vec![format!("vstr {vstr}")]);
        }
    }

    pub(in crate::app) fn japro_flipkick_restricted(&self) -> bool {
        self.server_restrictions()
            .contains(crate::app::restrictions::FLIPKICK)
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

    /// Zoom the chase-camera range relative to the same `cg_fov` reference
    /// used by the first-person projection. TaystJK's original range^2 / zoom
    /// makes equal cg_fov / cg_zoomFov values change camera distance whenever
    /// cg_thirdPersonRange differs from cg_fov (the reported regression).
    pub(in crate::app) fn japro_zoom_third_person_range(&self, range: f32) -> f32 {
        let zoom = self.japro_zoom_fov_integer().clamp(1, 176) as f32;
        range * (self.camera.cg_fov() / zoom)
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

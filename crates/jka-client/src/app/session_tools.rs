//! Session-level helpers behind the menus: spectator "follow fastest", the open
//! vote's HUD line and the in-game third-person camera editor.

use super::egui_menu::{SETUP_TAB_CAMERA, TOP_SETUP};
use super::*;

const PMF_FOLLOW: i32 = 4096;
const PERS_TEAM: usize = 3;
const TEAM_SPECTATOR: i32 = 3;
const PW_REDFLAG: i32 = 4;
const PW_BLUEFLAG: i32 = 5;

/// What the spectator can see of everyone's speed.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SpectateInfo {
    /// The followed client and its horizontal speed; `None` for a free camera.
    pub followed: Option<(i32, f32)>,
    /// Every other living player in the snapshot: `(client, horizontal speed)`.
    pub others: Vec<(i32, f32)>,
}

/// Which flag carrier `followRedFlag` / `followBlueFlag` look for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FlagCarrier {
    Red,
    Blue,
}

/// A monotonic millisecond clock for debouncing UI-level automation.
fn monotonic_ms() -> i32 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis().min(i32::MAX as u128) as i32
}

impl GameSession {
    /// `None` unless the viewer is a spectator (free or following).
    pub(super) fn spectate_info(&self) -> Option<SpectateInfo> {
        let ps = &self.current_snapshot.as_ref()?.player_state;
        let following = ps.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW != 0;
        if !following && ps.persistant[PERS_TEAM] != TEAM_SPECTATOR {
            return None;
        }
        let own = ps.field_i32("clientNum").unwrap_or(-1);
        let followed = following.then(|| {
            let speed = ps
                .field_f32("velocity[0]")
                .zip(ps.field_f32("velocity[1]"))
                .map_or(0.0, |(x, y)| x.hypot(y));
            (own, speed)
        });
        let others = self
            .presented_entities
            .iter()
            .filter(|entity| {
                entity.entity_type == crate::cgame::ET_PLAYER
                    && i32::from(entity.number) != own
                    && entity.state.field_i32("eFlags").unwrap_or(0) & 2 == 0 // EF_DEAD
            })
            .map(|entity| {
                let speed = entity
                    .state
                    .field_f32("pos.trDelta[0]")
                    .zip(entity.state.field_f32("pos.trDelta[1]"))
                    .map_or(0.0, |(x, y)| x.hypot(y));
                (i32::from(entity.number), speed)
            })
            .collect();
        Some(SpectateInfo { followed, others })
    }

    /// The client currently carrying `flag`, other than the one being followed.
    pub(super) fn flag_carrier(&self, flag: FlagCarrier) -> Option<i32> {
        let own = self
            .current_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.player_state.field_i32("clientNum"))
            .unwrap_or(-1);
        let bit = 1 << match flag {
            FlagCarrier::Red => PW_REDFLAG,
            FlagCarrier::Blue => PW_BLUEFLAG,
        };
        self.presented_entities
            .iter()
            .filter(|entity| entity.entity_type == crate::cgame::ET_PLAYER && i32::from(entity.number) != own)
            .find(|entity| entity.state.field_i32("powerups").unwrap_or(0) & bit != 0)
            .map(|entity| i32::from(entity.number))
    }
}

impl App {
    /// `CG_DrawVote`'s summary line while a vote is open.
    pub(super) fn vote_hud_line(&self) -> Option<String> {
        self.game_session
            .as_ref()
            .and_then(|session| session.vote_ui.as_ref())
            .map(crate::vote::VoteUi::hud_line)
    }

    fn spectating_live(&self) -> Option<SpectateInfo> {
        if !self.live_connected() {
            return None;
        }
        self.game_session.as_ref().filter(|session| session.live).and_then(GameSession::spectate_info)
    }

    /// Whether the connected client is currently a spectator.
    pub(super) fn is_spectating_live(&self) -> bool {
        self.spectating_live().is_some()
    }

    /// Spectator-specific binds are also the demo controls in every demo view.
    /// The map is sparse, so movement/weapon commands still inherit normal binds
    /// unless the user explicitly gave that key a spectator override.
    pub(super) fn spectator_binding_context(&self) -> bool {
        self.is_spectating_live() || self.demo_playback_active()
    }

    /// Orbit input only owns the mouse while actually following somebody. A
    /// free spectator keeps the ordinary noclip/free-look controls.
    pub(super) fn spectator_orbit_follow_active(&self) -> bool {
        let live_follow = self.spectating_live().is_some_and(|info| info.followed.is_some());
        let demo_follow = self.demo_playback_active() && self.demo_view_mode != DemoViewMode::Free;
        self.spectator_camera.mode == crate::camera::SpectatorCameraMode::Orbit
            && (live_follow || demo_follow)
    }

    /// jaPRO `followFastest`: follow whoever is moving fastest right now.
    pub(super) fn follow_fastest_now(&mut self) {
        let Some(info) = self.spectating_live() else {
            self.push_console_line("^3followFastest:^7 join the spectators on a server first");
            return;
        };
        let fastest = info
            .others
            .iter()
            .copied()
            .filter(|&(_, speed)| speed > 0.0)
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match fastest {
            Some((client, _)) => self.forward_command_to_server(&format!("follow {client}")),
            None => self.push_console_line("^3followFastest:^7 nobody is moving"),
        }
    }

    /// jaPRO `followRedFlag` / `followBlueFlag`.
    pub(super) fn follow_flag_carrier(&mut self, flag: FlagCarrier) {
        if !self.is_spectating_live() {
            self.push_console_line("^3follow:^7 join the spectators on a server first");
            return;
        }
        let carrier = self.game_session.as_ref().and_then(|session| session.flag_carrier(flag));
        match carrier {
            Some(client) => self.forward_command_to_server(&format!("follow {client}")),
            None => self.push_console_line("^3follow:^7 nobody is carrying that flag"),
        }
    }

    /// `cg_specFollowFastest`: called every frame; switches the followed player
    /// only when [`japro_cg::FollowFastest`] says a clear, sustained lead exists.
    pub(super) fn tick_follow_fastest(&mut self, _now: Instant) {
        if !self.japro_cg.follow_fastest {
            return;
        }
        let Some(info) = self.spectating_live() else {
            if let Some(session) = self.game_session.as_mut() {
                session.follow_fastest.reset();
            }
            return;
        };
        let now = monotonic_ms();
        let pick = self
            .game_session
            .as_mut()
            .and_then(|session| session.follow_fastest.update(now, info.followed, &info.others));
        if let Some(client) = pick {
            self.forward_command_to_server(&format!("follow {client}"));
        }
    }

    // ------------------------------------------------------ camera editor --

    /// Enter the in-game third-person camera editor.
    pub(super) fn start_camera_edit(&mut self) {
        if self.front_end || self.game_session.is_none() {
            self.push_console_line("^3cameraedit:^7 join a game (or start Solo Game) before adjusting the camera");
            return;
        }
        if self.camera_edit.is_none() {
            self.camera_edit = Some(egui_pages::CameraEditState { original: self.third_person });
        }
        // The editor always previews third person, and with no damping so the
        // camera follows the mouse exactly; both are put back when it closes.
        self.third_person.enabled = true;
        self.third_person.camera_damp = 1.0;
        self.third_person.target_damp = 1.0;
        self.third_person_camera.reset();
        self.set_overlay(OverlayMode::CameraEdit);
        self.update_solo_player_view_and_presentation();
        self.publish_snapshot();
    }

    /// Leave the camera editor, keeping (`save`) or discarding what was adjusted.
    pub(super) fn finish_camera_edit(&mut self, save: bool) {
        let Some(state) = self.camera_edit.take() else { return };
        let mut settings = state.original;
        if save {
            let edited = self.third_person;
            settings.range = edited.range;
            settings.horz_offset = edited.horz_offset;
            settings.vert_offset = edited.vert_offset;
            settings.angle = edited.angle;
            settings.pitch_offset = edited.pitch_offset;
            self.mark_config_dirty();
        }
        self.third_person = settings;
        self.third_person_camera.reset();
        self.menu_selected = TOP_SETUP;
        self.setup_selected = SETUP_TAB_CAMERA;
        self.set_overlay(OverlayMode::Video);
        self.update_solo_player_view_and_presentation();
        self.publish_snapshot();
    }

    /// Put the camera back to OpenJK's defaults.
    pub(super) fn reset_camera_view(&mut self) {
        let defaults = ThirdPersonSettings::default();
        self.third_person.range = defaults.range;
        self.third_person.horz_offset = defaults.horz_offset;
        self.third_person.vert_offset = defaults.vert_offset;
        self.third_person.angle = defaults.angle;
        self.third_person.pitch_offset = defaults.pitch_offset;
        // A reset inside the editor stays cancellable, so it is only saved
        // when the editor is not open.
        if self.camera_edit.is_none() {
            self.mark_config_dirty();
        }
        self.third_person_camera.reset();
        self.publish_snapshot();
    }
}

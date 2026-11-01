//! Shared Profile Studio helpers and the server's Force limits. The Force
//! page itself lives in `force_page.rs`.
use crate::app::egui_menu::{ui_catalog, App, ProfileModelEntry, ProfileSaberEntry};

impl App {
    // Shared asset-label helpers for the single Profile UI.

    pub(in crate::app::egui_menu) fn profile_model_icon_texture(
        &mut self,
        entry: &ProfileModelEntry,
    ) -> Option<egui::TextureHandle> {
        let key = entry.value.to_ascii_lowercase();
        if let Some(texture) = self.profile_model_icon_textures.get(&key) {
            return Some(texture.clone());
        }
        if self.ui_catalog.icons_missing.contains(&key)
            || self.ui_catalog.icons_inflight.contains(&key)
        {
            return None;
        }
        self.ui_catalog.icons_inflight.insert(key.clone());
        self.ui_catalog.request(
            &self.base,
            self.game.as_deref(),
            ui_catalog::CatalogRequest::ProfileIcon {
                key,
                model: entry.model_name.clone(),
                skin: entry.skin_name.clone(),
            },
        );
        None
    }

    pub(in crate::app::egui_menu) fn profile_saber_label(entry: &ProfileSaberEntry) -> &str {
        // Stock saber files may use @MENUS_* localization tokens. Until the
        // string-package browser is wired into egui, the block identifier is a
        // cleaner fallback than exposing the raw token.
        if entry.display_name.is_empty() || entry.display_name.starts_with('@') {
            &entry.name
        } else {
            &entry.display_name
        }
    }

    pub(in crate::app::egui_menu) fn profile_saber_is_single(entry: &ProfileSaberEntry) -> bool {
        entry.saber_type.eq_ignore_ascii_case("SABER_SINGLE")
    }

    pub(in crate::app::egui_menu) fn profile_saber_is_staff(entry: &ProfileSaberEntry) -> bool {
        entry.saber_type.eq_ignore_ascii_case("SABER_STAFF")
    }

    pub(in crate::app::egui_menu) fn profile_server_force_limits(
        &self,
    ) -> (u8, u32, i32, bool, Option<u8>) {
        let info = if let Some(net) = self.net.as_ref() {
            crate::net::mod_support::server_info(&net.session().decoder().configstrings)
        } else if let Some(server) = self.local_server.as_ref() {
            server
                .configstrings()
                .get(&crate::cgame::CS_SERVERINFO)
                .map(Vec::as_slice)
                .unwrap_or_default()
        } else {
            &[]
        };
        let value = |key: &[u8], fallback: i32| {
            jka_protocol::commands::info_value(info, key)
                .map(jka_protocol::commands::atoi)
                .unwrap_or(fallback)
        };
        let max_rank = value(b"g_maxForceRank", 7).clamp(0, 7) as u8;
        let disabled = value(b"g_forcePowerDisable", 0) as u32;
        let gametype = value(b"g_gametype", 0);

        // TaystJK UI_HasSetSaberOnly, ported verbatim in behavior. Jedi Master
        // never grants the free saber; duel modes use g_duelWeaponDisable and
        // other modes use g_weaponDisable. WP_NONE=0, WP_SABER=3, WP_NUM=19.
        let free_saber = if gametype == 2 {
            false
        } else {
            let weapon_disable = if matches!(gametype, 3 | 4) {
                value(b"g_duelWeaponDisable", 0)
            } else {
                value(b"g_weaponDisable", 0)
            } as u32;
            (0..19).all(|weapon| {
                weapon == 0 || weapon == 3 || (weapon_disable & (1u32 << weapon)) != 0
            })
        };

        // UI_DrawForceSide: force-based teams pin Red to Dark and Blue to Light.
        let forced_side = if value(b"g_forceBasedTeams", 0) != 0 {
            self.game_session.as_ref().and_then(|session| {
                let client_num = session
                    .current_snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot.player_state.field_i32("clientNum"))
                    .and_then(|client| usize::try_from(client).ok())?;
                let team = session
                    .client_game
                    .client_info(client_num, &session.siege_classes)?
                    .team;
                match team {
                    1 => Some(2), // TEAM_RED -> FORCE_DARKSIDE
                    2 => Some(1), // TEAM_BLUE -> FORCE_LIGHTSIDE
                    _ => None,
                }
            })
        } else {
            None
        };
        (max_rank, disabled, gametype, free_saber, forced_side)
    }


}

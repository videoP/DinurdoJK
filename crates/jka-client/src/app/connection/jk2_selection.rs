//! JK2 CGame weapon / Force / inventory selection commands. JK2 connections
//! run the pinned MVSDK commands on raw wire states; the HUD keeps reading
//! LiveInput's presentation mirrors.
use crate::app::{App, Duration, GameCommand, Instant};
use jka_movement::jk2::select::Outcome;

impl App {
    /// cg.snap->ps and cg.predictedPlayerState as raw JK2 wire states, cg.time
    /// and the presented snapshot state, for the current JK2 live frame.
    pub(in crate::app) fn jk2_selection_frame(&self) -> Option<crate::net::Jk2Frame> {
        use crate::net::jk2_prediction::{gameplay, import};
        let session = self.net.as_ref()?.session();
        let version = session.jk2_version()?;
        let wire = session.jk2_decoder()?;
        let current = self.game_session.as_ref().filter(|s| s.live)?.current_snapshot.as_ref()?;
        let snap = import(version, &wire.snapshot(current.message_num)?.player_state).ok()?;
        let gameplay = gameplay(version, &wire.configstrings, session.client_num()).ok()?;
        let predicted = self
            .predictor
            .jk2
            .as_ref()
            .and_then(|predictor| predictor.predicted_state())
            .filter(|state| state.version == snap.version)
            .unwrap_or_else(|| snap.clone());
        Some(crate::net::Jk2Frame {
            gameplay,
            snap,
            predicted,
            time: session.server_time(),
            presented: current.player_state.clone(),
        })
    }

    /// CG_ServerCommand `nfr` (ui_rankChange / ui_myteam): WP_InitForcePowers
    /// returned the client to spectating, either for its first spawn or for a
    /// `forcepowers` string above the server's rank. JK2's UI legalizes the
    /// configuration for that rank before the player joins again; do the same
    /// with the pinned BG_LegalizedForcePowers and the server's own inputs.
    pub(in crate::app) fn handle_jk2_force_rank(&mut self) {
        use jka_movement::jk2::select::{legalize_force_powers, ForceRules};
        if !self.live_input.is_jk2() {
            return;
        }
        let ranks = self
            .game_session
            .as_mut()
            .map(|session| session.client_game.drain_new_force_ranks())
            .unwrap_or_default();
        let Some(&(rank, _, team)) = ranks.last() else { return };
        let Some((version, gameplay, info)) = self.net.as_ref().and_then(|net| {
            let session = net.session();
            let wire = session.jk2_decoder()?;
            let gameplay =
                crate::net::jk2_prediction::gameplay(session.jk2_version()?, &wire.configstrings, session.client_num())
                    .ok()?;
            Some((session.jk2_version()?, gameplay, wire.configstrings.get(&0).cloned().unwrap_or_default()))
        }) else {
            return;
        };
        let value = |key: &[u8]| {
            jka_protocol::commands::info_value(&info, key).map_or(0, jka_protocol::commands::atoi)
        };
        const GT_JEDIMASTER: i32 = 2;
        const GT_TOURNAMENT: i32 = 3;
        let gametype = value(b"g_gametype");
        // w_saber.c HasSetSaberOnly over JK2's 16 weapons.
        let disabled = value(if gametype == GT_TOURNAMENT { b"g_duelWeaponDisable".as_slice() } else { b"g_weaponDisable" });
        let free_saber = gametype != GT_JEDIMASTER && (1..16).filter(|&weapon| weapon != 2).all(|weapon| disabled & (1 << weapon) != 0);
        // g_forceBasedTeams: red is dark side (2), blue light side (1).
        let team_force = match (value(b"g_forceBasedTeams") != 0, team) {
            (true, 1) => 2,
            (true, 2) => 1,
            _ => 0,
        };
        let rules = ForceRules { max_rank: rank, free_saber, team_force, gametype, force_disable: value(b"g_forcePowerDisable") };
        let current = self.jk2_effective_forcepowers();
        let native = |v: jka_protocol::jk2::Version| match v {
            jka_protocol::jk2::Version::V102 => jka_movement::jk2::Version::V102,
            jka_protocol::jk2::Version::V104 => jka_movement::jk2::Version::V104,
        };
        match legalize_force_powers(native(version), gameplay, &current, rules) {
            Ok((legal, true)) => {
                self.push_console_line(format!(
                    "^3Server force rank {rank}: force powers {legal} accepted; use /team free (or a team) to join."
                ));
            }
            Ok((legal, false)) => {
                self.jk2_forcepowers = Some((self.network.forcepowers.clone(), legal.clone()));
                if let Err(error) = self.queue_userinfo() {
                    self.push_console_line(format!("^1Could not send JK2 force powers: {error}"));
                    return;
                }
                self.push_console_line(format!(
                    "^3Server force rank {rank}: force powers adjusted to {legal} for this server; use /team free (or a team) to join."
                ));
            }
            Err(error) => self.push_console_line(format!("^1JK2 force rank {rank}: {error}")),
        }
    }

    /// The `forcepowers` a JK2 server receives: the rank-legalized string while
    /// the setting it came from is unchanged, else the setting as chosen.
    pub(in crate::app) fn jk2_effective_forcepowers(&self) -> String {
        match &self.jk2_forcepowers {
            Some((source, legal)) if *source == self.network.forcepowers => legal.clone(),
            _ => self.network.forcepowers.clone(),
        }
    }

    /// Selection commands on a JK2 connection. Returns false for commands
    /// that keep their game-independent client behaviour.
    pub(in crate::app) fn execute_jk2_selection(&mut self, action: GameCommand, words: &[&str]) -> bool {
        if !self.live_input.is_jk2() {
            return false;
        }
        let name = words.first().copied().unwrap_or_default();
        match action {
            GameCommand::ToggleFireMode => return false,
            GameCommand::WeaponLast | GameCommand::WeaponClean | GameCommand::InventoryFree => {
                self.push_console_line(format!("^3{name}: not a JK2 CGame command"));
                return true;
            }
            _ => {}
        }
        // No snapshot yet, or deferred 1.03 gameplay: CGame has nothing to select from.
        let Some(frame) = self.jk2_selection_frame() else { return true };
        let result = match action {
            GameCommand::WeaponNext | GameCommand::WeaponPrev => {
                self.live_input.jk2_cycle_weapon(&frame, matches!(action, GameCommand::WeaponNext))
            }
            GameCommand::Weapon => {
                let slot = words.get(1).map_or(0, |slot| jka_protocol::commands::atoi(slot.as_bytes()));
                self.live_input.jk2_weapon_slot(&frame, slot)
            }
            GameCommand::ForceNext | GameCommand::ForcePrev => {
                self.live_input.jk2_cycle_force(&frame, matches!(action, GameCommand::ForceNext))
            }
            GameCommand::InventoryNext | GameCommand::InventoryPrev => {
                self.live_input.jk2_cycle_inventory(&frame, matches!(action, GameCommand::InventoryNext))
            }
            _ => return false,
        };
        match result {
            Ok(Outcome::Selected) => {
                let until = Some(Instant::now() + Duration::from_millis(1530));
                match action {
                    GameCommand::ForceNext | GameCommand::ForcePrev => {
                        self.force_select_until = until;
                        self.weapon_select_until = None;
                        self.inventory_select_until = None;
                    }
                    GameCommand::InventoryNext | GameCommand::InventoryPrev => {
                        self.show_inventory_selection(&frame.presented);
                        return true;
                    }
                    _ => {
                        self.weapon_select_until = until;
                        self.force_select_until = None;
                        self.inventory_select_until = None;
                    }
                }
                self.publish_transient_ui();
            }
            Ok(_) => {}
            Err(error) => self.push_console_line(format!("^1{name}: {error}")),
        }
        true
    }
}

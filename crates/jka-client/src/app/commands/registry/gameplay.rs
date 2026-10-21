//! Declarative command handlers.
use crate::app::{scene, App, Arc, DemoViewMode, JoinMode};

use super::{CommandDef, CommandResult, CommandScope, GameCommand};
commands! {
    cmd_weapnext ["weapnext", ] ("weapnext", "Select the next weapon.", CommandScope::Always, game = GameCommand::WeaponNext) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        if self.execute_game_command(command) {
            return CommandResult::Handled;
        }
        match words.as_slice() {
            [_verb] if self.demo_playback_active() => {
                // Like JKA/Tayst cgame, a registered weapon command stays consumed
                // while spectating/following even when it has no weapon-selection
                // effect. DinurdoJK additionally repurposes it in demo Free view.
                if self.demo_view_mode == DemoViewMode::Free {
                    // Weapon-next / wheel-down slows free movement.
                    self.adjust_demo_free_speed(-1);
                }
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_weapprev ["weapprev", ] ("weapprev", "Select the previous weapon.", CommandScope::Always, game = GameCommand::WeaponPrev) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        if self.execute_game_command(command) {
            return CommandResult::Handled;
        }
        match words.as_slice() {
            [_verb] if self.demo_playback_active() => {
                // Keep the command recognized in every demo camera mode. Only
                // Free view gives it DinurdoJK's camera-speed side effect.
                if self.demo_view_mode == DemoViewMode::Free {
                    // Weapon-prev / wheel-up speeds free movement up.
                    self.adjust_demo_free_speed(1);
                }
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    // TaystJK cg_consolecmds.c / cg_weapons.c; all remain cgame-local.
    cmd_weaplast ["weaplast", ] ("weaplast", "Switch back to the previous selected weapon.", CommandScope::Always, game = GameCommand::WeaponLast) |self, command| {
        if self.execute_game_command(command) { CommandResult::Handled } else { CommandResult::Unhandled }
    }
    cmd_weaponclean ["weaponclean", ] ("weaponclean <raw_weapon_id>", "Choose a weapon by raw WP_* ID (without /weapon slot remapping).", CommandScope::Always, game = GameCommand::WeaponClean) |self, command| {
        if self.execute_game_command(command) { CommandResult::Handled } else { CommandResult::Unhandled }
    }
    cmd_selectfiremode ["selectfiremode", ] ("selectfiremode", "TaystJK: toggle singlefire primary/alternate firing mode.", CommandScope::Always, game = GameCommand::ToggleFireMode) |self, command| {
        if self.execute_game_command(command) { CommandResult::Handled } else { CommandResult::Unhandled }
    }
    cmd_tell_target ["tell_target", ] ("tell_target <message>", "Send private text to the crosshair target (TaystJK CG_TellTarget_f).", CommandScope::Always) |self, command| {
        self.tell_selected_player(command, false);
        CommandResult::Handled
    }
    cmd_tell_attacker ["tell_attacker", ] ("tell_attacker <message>", "Send private text to the last player who damaged you (TaystJK CG_TellAttacker_f).", CommandScope::Always) |self, command| {
        self.tell_selected_player(command, true);
        CommandResult::Handled
    }
    // +singlefire / +duck / +grapple are holdable commands: release companions
    // are registered and exposed to the keybind release generator.
    cmd_singlefire_down ["+singlefire", ] ("+singlefire", "Fire with selected primary/alternate fire mode.", CommandScope::Always) |self, command| {
        self.tayst_special_button(command); CommandResult::Handled
    }
    cmd_singlefire_up ["-singlefire", ] ("-singlefire", "Release singlefire.", CommandScope::Always) |self, command| {
        self.tayst_special_button(command); CommandResult::Handled
    }
    cmd_duck_down ["+duck", ] ("+duck", "TaystJK no-roll duck sequence.", CommandScope::Always) |self, command| {
        self.tayst_special_button(command); CommandResult::Handled
    }
    cmd_duck_up ["-duck", ] ("-duck", "Release TaystJK no-roll duck.", CommandScope::Always) |self, command| {
        self.tayst_special_button(command); CommandResult::Handled
    }
    cmd_grapple_down ["+grapple", ] ("+grapple", "TaystJK grapple button12.", CommandScope::Always) |self, command| {
        self.tayst_special_button(command); CommandResult::Handled
    }
    cmd_grapple_up ["-grapple", ] ("-grapple", "Release TaystJK grapple; JA+ performs use pulse.", CommandScope::Always) |self, command| {
        self.tayst_special_button(command); CommandResult::Handled
    }
    cmd_lowjump ["lowjump", ] ("lowjump", "TaystJK brief jump tap.", CommandScope::Always) |self, command| {
        self.tayst_lowjump(); CommandResult::Handled
    }

    cmd_follownext ["follownext", ] ("follownext", "Spectate the next player.", CommandScope::Connected) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        if self.execute_game_command(command) {
            return CommandResult::Handled;
        }
        match words.as_slice() {
            [_verb] if self.demo_playback_active() => {
                self.cycle_demo_follow(1);
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_followprev ["followprev", ] ("followprev", "Spectate the previous player.", CommandScope::Connected) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        if self.execute_game_command(command) {
            return CommandResult::Handled;
        }
        match words.as_slice() {
            [_verb] if self.demo_playback_active() => {
                self.cycle_demo_follow(-1);
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_devmap ["devmap", "map"] ("devmap <map>", "Load a map for local development: devmap <map>.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, name] => {
                match scene::MapSource::from_map_argument(name) {
                    Ok(source) => match scene::verify_map_source_exists(
                        &self.base,
                        self.game.as_deref(),
                        &source,
                    ) {
                        Ok(()) => {
                            // A local map/devmap launch owns the client state. Tear down any
                            // live netchan first so server snapshots/commands cannot continue
                            // arriving while the local map is being prepared.
                            if self.net.is_some() {
                                self.disconnect_to_main_menu();
                            } else {
                                // `map`/`devmap` starts a new local server lifetime.
                                // Do not let the previous local/demo CGame make the
                                // MapPrepared handler think an external authority
                                // already owns the new world.
                                self.game_session = None;
                                self.local_server = None;
                                self.predictor.reset();
                                self.solo_dynamic_models = Arc::new(Vec::new());
                            }
                            self.enter_local_game_dir();
                            // Only `menudevmap` keeps the main-menu look for the next load.
                            self.menu_look_play = std::mem::take(&mut self.menu_look_oneshot);
                            self.request_map(source);
                            self.console_map_barrier = !self.console_command_buffer.is_empty();
                        }
                        Err(error) => {
                            self.console_status = error.to_ascii_uppercase();
                        }
                    },
                    Err(error) => {
                        self.console_status = error.to_ascii_uppercase();
                    }
                }
                CommandResult::Status
            }
            [_verb] => {
                self.console_status = "USAGE: DEVMAP MP/FFA3[.BSP|.MAP]".into();
                CommandResult::Status
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_force_speed ["force_speed", ] ("force_speed", "Use Force speed.", CommandScope::Always, game = GameCommand::Generic(4)) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        if self.execute_game_command(command) {
            return CommandResult::Handled;
        }
        match words.as_slice() {
            [_verb] => {
                if let Some(player) = &mut self.local_server {
                    player.request_power(jka_movement::MovementPower::Speed);
                }
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_force_rage ["force_rage", ] ("force_rage", "Use Force rage.", CommandScope::Always, game = GameCommand::Generic(8)) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        if self.execute_game_command(command) {
            return CommandResult::Handled;
        }
        match words.as_slice() {
            [_verb] => {
                if let Some(player) = &mut self.local_server {
                    player.request_power(jka_movement::MovementPower::Rage);
                }
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_team ["team", ] ("team [team]", "Show or change team: team [free|f|red|r|blue|b|spectator|spectate|spec|s|follow1|follow2|scoreboard].", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        if self.execute_game_command(command) {
            return CommandResult::Handled;
        }
        match words.as_slice() {
            [_verb] if self.local_server.is_some() => {
                // OpenJK Cmd_Team_f with no argv(1) prints the current team.
                self.local_server
                    .as_mut()
                    .expect("checked above")
                    .report_current_team();
                self.push_local_snapshot(true);
                CommandResult::Handled
            }
            [_verb, requested_team] if self.local_server.is_some() => {
                // OpenJK SetTeam in GT_FFA: scoreboard/follow/spectator forms become
                // TEAM_SPECTATOR; every other token becomes TEAM_FREE. Keep that
                // fallback, while also accepting the common `spectate`/`spec` spellings
                // requested by the local shim.
                let mode = if matches!(
                    requested_team.to_ascii_lowercase().as_str(),
                    "scoreboard"
                        | "score"
                        | "follow1"
                        | "follow2"
                        | "spectator"
                        | "spectate"
                        | "spec"
                        | "s"
                ) {
                    JoinMode::Spectator
                } else {
                    JoinMode::Player
                };
                self.join_as(mode);
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_give ["give", ] ("give all", "Cheat: give <all|health|armor|ammo|weapons|force|item name>.", CommandScope::Connected) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        if self.execute_game_command(command) {
            return CommandResult::Handled;
        }
        match words.as_slice() {
            [_verb, what] if what.eq_ignore_ascii_case("all") && self.local_server.is_some() => {
                let result = self
                    .local_server
                    .as_mut()
                    .expect("checked above")
                    .give_all();
                match result {
                    Ok(()) => {
                        self.push_local_snapshot(true);
                        self.update_solo_player_view_and_presentation();
                        self.publish_ui();
                    }
                    Err(error) => self.push_console_line(format!("^1{error}")),
                }
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_setviewpos ["setviewpos", ] ("setviewpos <x> <y> <z> <yaw>", "Cheat: teleport: setviewpos <x> <y> <z> <yaw>.", CommandScope::Connected) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        if self.execute_game_command(command) {
            return CommandResult::Handled;
        }
        match words.as_slice() {
            [_verb, arguments @ ..] if self.local_server.is_some() => {
                self.local_setviewpos(arguments);
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_setviewpos_exact ["setviewpos_exact", ] ("setviewpos_exact <x> <y> <z> <yaw> [pitch]", "Local game: setviewpos without the 400 u/s teleport push, so the view rests exactly there (benchmarks).", CommandScope::Connected) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb, arguments @ ..] if self.local_server.is_some() => {
                self.local_setviewpos_with_push(arguments, 1);
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_cycle_spawn ["cycle_spawn", ] ("cycle_spawn", "Cycle the local spawn point.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        match words.as_slice() {
            [_verb] => {
                self.cycle_spawn();
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_noclip ["noclip", ] ("noclip", "Cheat: toggle noclip movement.", CommandScope::Always) |self, command| {
        let words: Vec<_> = command.split_whitespace().collect();
        if self.execute_game_command(command) {
            return CommandResult::Handled;
        }
        match words.as_slice() {
            [_verb] => {
                self.toggle_noclip();
                CommandResult::Handled
            }
            _ => return CommandResult::Unhandled,
        }
    }

    cmd_game_weapon ["weapon", ] ("weapon <1..13>", "Select a weapon slot: weapon <1..13>.", CommandScope::Always, game = GameCommand::Weapon) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_forcenext ["forcenext", ] ("forcenext", "Select the next usable Force power.", CommandScope::Always, game = GameCommand::ForceNext) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_forceprev ["forceprev", ] ("forceprev", "Select the previous usable Force power.", CommandScope::Always, game = GameCommand::ForcePrev) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    // TaystJK cg_consolecmds.c / cg_main.c: these operate on the local
    // holdable selector, never forward as unknown server commands.
    cmd_game_invnext ["invnext", ] ("invnext", "Select the next owned, selectable inventory item.", CommandScope::Always, game = GameCommand::InventoryNext) |self, command| {
        if self.execute_game_command(command) { CommandResult::Handled } else { CommandResult::Unhandled }
    }

    cmd_game_invprev ["invprev", ] ("invprev", "Select the previous owned, selectable inventory item.", CommandScope::Always, game = GameCommand::InventoryPrev) |self, command| {
        if self.execute_game_command(command) { CommandResult::Handled } else { CommandResult::Unhandled }
    }

    cmd_game_invfree ["invfree", ] ("invfree", "TaystJK: use the first owned holdable other than jetpack.", CommandScope::Always, game = GameCommand::InventoryFree) |self, command| {
        if self.execute_game_command(command) { CommandResult::Handled } else { CommandResult::Unhandled }
    }

    cmd_game_flipkick ["flipkick", ] ("flipkick", "TaystJK jaPRO flipkick bind sequence.", CommandScope::Always, game = GameCommand::Flipkick) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_japro_do ["do", ] ("do <vstr> <delay_ms>", "jaPRO: asynchronously execute one vstr after a millisecond delay.", CommandScope::Always) |self, command| {
        let args = jka_protocol::commands::tokenize(command.as_bytes());
        if args.len() != 3 {
            self.push_console_line("Usage: /do <vstr> <delay>".to_owned());
            return CommandResult::Handled;
        }
        if self.game_session.is_none() {
            return CommandResult::Unhandled;
        }
        if self.japro_do_restricted() {
            return CommandResult::Handled;
        }

        // jaPRO stores both the argv value and the resulting "vstr <name>\n"
        // in MAX_QPATH (64-byte) buffers. Com_sprintf may drop the newline at
        // the boundary, so the effective vstr-name payload is 58 bytes.
        const MAX_QPATH: usize = 64;
        const PREFIX_LEN: usize = 5; // "vstr "
        let max_name = MAX_QPATH - 1 - PREFIX_LEN;
        let name_bytes = &args[1][..args[1].len().min(max_name)];
        let vstr = String::from_utf8_lossy(name_bytes).into_owned();

        // The jaPRO source clearly intends `delayMS < 0`; it accidentally
        // compares the delay character buffer itself. Clamp the parsed value
        // to the intended 0..1 hour range while retaining C atoi semantics.
        let delay_ms = jka_protocol::commands::atoi(&args[2]).clamp(0, 1000 * 60 * 60);
        self.start_japro_do(vstr, delay_ms);
        CommandResult::Handled
    }

    cmd_game_sv_saberswitch ["sv_saberswitch", ] ("sv_saberswitch", "Holster/activate lightsaber.", CommandScope::Always, game = GameCommand::Generic(1)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_engage_duel ["engage_duel", ] ("engage_duel", "Challenge the player you are looking at to a private duel.", CommandScope::Always, game = GameCommand::Generic(2)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_force_heal ["force_heal", ] ("force_heal", "Use Force heal.", CommandScope::Always, game = GameCommand::Generic(3)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_force_throw ["force_throw", ] ("force_throw", "Use Force push.", CommandScope::Always, game = GameCommand::Generic(5)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_force_pull ["force_pull", ] ("force_pull", "Use Force pull.", CommandScope::Always, game = GameCommand::Generic(6)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_force_distract ["force_distract", ] ("force_distract", "Activate Force mind trick.", CommandScope::Always, game = GameCommand::Generic(7)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_force_protect ["force_protect", ] ("force_protect", "Activate Force protect.", CommandScope::Always, game = GameCommand::Generic(9)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_force_absorb ["force_absorb", ] ("force_absorb", "Activate Force absorb.", CommandScope::Always, game = GameCommand::Generic(10)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_force_healother ["force_healother", ] ("force_healother", "Use team heal.", CommandScope::Always, game = GameCommand::Generic(11)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_force_forcepowerother ["force_forcepowerother", ] ("force_forcepowerother", "Use team energize.", CommandScope::Always, game = GameCommand::Generic(12)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_force_seeing ["force_seeing", ] ("force_seeing", "Activate Force seeing.", CommandScope::Always, game = GameCommand::Generic(13)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_use_seeker ["use_seeker", ] ("use_seeker", "Use seeker drone.", CommandScope::Always, game = GameCommand::Generic(14)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_use_field ["use_field", ] ("use_field", "Use forcefield.", CommandScope::Always, game = GameCommand::Generic(15)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_use_bacta ["use_bacta", ] ("use_bacta", "Use bacta.", CommandScope::Always, game = GameCommand::Generic(16)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_use_electrobinoculars ["use_electrobinoculars", ] ("use_electrobinoculars", "Use electro binoculars.", CommandScope::Always, game = GameCommand::Generic(17)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_use_sentry ["use_sentry", ] ("use_sentry", "Use sentry gun.", CommandScope::Always, game = GameCommand::Generic(19)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_use_jetpack ["use_jetpack", ] ("use_jetpack", "Use jetpack.", CommandScope::Always, game = GameCommand::Generic(20)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_use_bactabig ["use_bactabig", ] ("use_bactabig", "Use big bacta.", CommandScope::Always, game = GameCommand::Generic(21)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_use_healthdisp ["use_healthdisp", ] ("use_healthdisp", "Use health dispenser.", CommandScope::Always, game = GameCommand::Generic(22)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_use_ammodisp ["use_ammodisp", ] ("use_ammodisp", "Use ammo dispenser.", CommandScope::Always, game = GameCommand::Generic(23)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_use_eweb ["use_eweb", ] ("use_eweb", "Use E-Web.", CommandScope::Always, game = GameCommand::Generic(24)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_use_cloak ["use_cloak", ] ("use_cloak", "Use cloaking device.", CommandScope::Always, game = GameCommand::Generic(25)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_saberattackcycle ["saberAttackCycle", ] ("saberAttackCycle", "Cycle lightsaber attack styles.", CommandScope::Always, game = GameCommand::Generic(jka_movement::GENCMD_SABERATTACKCYCLE)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_emote ["emote", ] ("emote", "Hold bound key/button for the tiered Emote wheel. Left click enters a category or plays an emote; release the bind to cancel.", CommandScope::Always) |self, command| {
        match command.split_whitespace().collect::<Vec<_>>().as_slice() {
            [_verb] => {
                self.toggle_emote_radial();
                CommandResult::Handled
            }
            _ => CommandResult::Unhandled,
        }
    }

    cmd_emote_net_test ["emote_net_test", ] ("emote_net_test <id>", "Send a one-snapshot Fortnite emote ID using a normal taunt (experimental).", CommandScope::Always) |self, command| {
        let id = command.split_whitespace().nth(1).and_then(|value| value.parse::<u16>().ok());
        match id {
            Some(id) if self.live_connected() && self.live_input.queue_emote_taunt(id) => {
                self.push_console_line(format!("^5[DJK EMOTE NET] transmitting ID {id} with immediate taunt (experimental)"));
            }
            _ => self.push_console_line("^3Usage: emote_net_test <0..8191> while connected to a server (8191 = stop)".to_owned()),
        }
        CommandResult::Handled
    }

    cmd_game_taunt ["taunt", ] ("taunt", "Taunt.", CommandScope::Always, game = GameCommand::Generic(27)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_bow ["bow", ] ("bow", "Bow.", CommandScope::Always, game = GameCommand::Generic(28)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_meditate ["meditate", ] ("meditate", "Meditate.", CommandScope::Always, game = GameCommand::Generic(29)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_flourish ["flourish", ] ("flourish", "Flourish.", CommandScope::Always, game = GameCommand::Generic(30)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_gloat ["gloat", ] ("gloat", "Gloat.", CommandScope::Always, game = GameCommand::Generic(31)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_zoom ["zoom", ] ("zoom", "Toggle binocular zoom.", CommandScope::Connected, game = GameCommand::Generic(18)) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

    cmd_game_amrun ["amRun", ] ("amRun", "jaPRO: Toggle the new run animation preference.", CommandScope::Japro, game = GameCommand::AmRun) |self, command| {
        if self.execute_game_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }

}

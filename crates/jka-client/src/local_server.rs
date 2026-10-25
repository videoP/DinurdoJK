//! Lightweight in-process server shim for `/map` and `/devmap`.
//!
//! The local authority still reuses the existing vendored-OpenJK Pmove host,
//! but it no longer owns a separate rendering path.  Instead it publishes the
//! same decoded protocol objects (`configstrings` + `Snapshot`) that a remote
//! server produces after netchan decoding.  CGame/presentation therefore sees
//! local and remote play through the same boundary.

mod game;
mod movers;

use std::{
    collections::{BTreeMap, HashSet, VecDeque},
    ops::{Deref, DerefMut},
    time::Duration,
};

use winit::keyboard::KeyCode;

use jka_assets::saber::{SaberDefinition, SaberDefinitions};

use crate::{
    camera::Camera,
    cgame::{ambient_sets::CS_AMBIENT_SET, ClientInfo, CS_EFFECTS, CS_MODELS, CS_MUSIC, CS_PLAYERS, CS_SERVERINFO, ET_FX},
    player::{LocalPlayer, MouseInputSettings},
    scene::{MapBrushEntity, MapFxRunner, MapWorldEffectRegistration, SpawnPoint},
    surface_deformation::SurfaceDeformationStamp,
};
use jka_movement::{
    CollisionWorld, JoinMode, MovementPower, PlayerEntityView, PlayerView, PmoveContext,
    SaberMovementInfo,
};
use jka_protocol::{
    gamestate::{EntityState, ENTITY_FIELDS},
    server::{PlayerState as ProtocolPlayerState, ServerCommand, Snapshot},
    session::CS_SYSTEMINFO,
};

const FX_STATE_OFF: i32 = 0;
const FX_STATE_CONTINUOUS: i32 = 20;
const FX_RUNNER_STARTOFF: i32 = 1;
const FX_RUNNER_ONESHOT: i32 = 2;
const MAX_LOCAL_EFFECTS: usize = 63;
const LOCAL_MAP_ENTITY_BASE: u16 = 64;
const MAX_GENTITIES: u16 = 1024;

// bg_public.h team_t / gametype_t values used by TaystJK's SetTeam and
// BroadcastTeamChange. The local authority is GT_FFA today, but keeping the
// whole branch table here avoids changing the announcement semantics when
// local team gametypes are added.
const TEAM_FREE: i32 = 0;
const TEAM_RED: i32 = 1;
const TEAM_BLUE: i32 = 2;
const TEAM_SPECTATOR: i32 = 3;
const GT_FFA: i32 = 0;
const GT_DUEL: i32 = 3;
const GT_POWERDUEL: i32 = 4;
const GT_TEAM: i32 = 6;
const GT_SIEGE: i32 = 7;

/// A one-client authoritative local host.  There is deliberately no fake UDP
/// socket or protocol-26 re-encoding here: this sits immediately after the
/// network decoder and feeds the exact semantic objects consumed by CGame.
pub struct LocalServer {
    diagnostic_epoch: std::time::Instant,
    player: LocalPlayer,
    map_name: String,
    client_name: String,
    worldspawn_music: String,
    configstrings: BTreeMap<u16, Vec<u8>>,
    /// Reliable server commands emitted by the in-process game authority.
    /// They are fed through CGame's ordinary `CG_ServerCommand` queue rather
    /// than turned into solo-only UI events.
    pending_server_commands: VecDeque<ServerCommand>,
    server_command_sequence: i32,
    map_effects: Vec<String>,
    /// The fx_runner entities.
    fx_entities: Vec<EntityState>,
    /// Doors, plats, triggers and every other brush entity, running on the
    /// player's timeline.
    game: game::MoverGame,
    /// `model2` names published as CS_MODELS entries 1..
    map_models: Vec<String>,
    next_message_num: i32,
    /// Offset maps Pmove's commandTime into a monotonic server timeline. Pmove
    /// player states legitimately reset commandTime on join/respawn; snapshot
    /// serverTime must never run backwards across those state transitions.
    player_time_offset: i64,
    last_snapshot_time: Option<i32>,
    /// Point -> portal area lookup for SV_BuildClientSnapshot's areamask.
    areas: Option<jka_assets::bsp::AreaLocator>,
    /// `ps.customRGBA`: G_ClientUserinfoChanged builds it from `char_color_*`
    /// with alpha 255. The player shim leaves it zero, so it is stamped here.
    custom_rgba: [i32; 4],
}

impl LocalServer {
    pub fn new(
        map_name: &str,
        movement: Option<PmoveContext>,
        world: Option<CollisionWorld>,
        spawn: SpawnPoint,
        client_info: &ClientInfo,
        saber_movement: [SaberMovementInfo; 2],
        fx_runners: &[MapFxRunner],
        world_effect_registrations: &[MapWorldEffectRegistration],
        brush_entities: &[MapBrushEntity],
        worldspawn_music: Option<&str>,
        areas: Option<jka_assets::bsp::AreaLocator>,
    ) -> Result<Self, String> {
        let mut player = LocalPlayer::new(movement, world, spawn)?;
        player.set_saber_movement_info(saber_movement)?;
        let map_name = normalize_map_name(map_name);
        let (map_effects, fx_entities) =
            build_map_fx_entities(fx_runners, world_effect_registrations);
        let movers = movers::build_brush_mover_entities(
            brush_entities,
            LOCAL_MAP_ENTITY_BASE.saturating_add(fx_entities.len() as u16),
            MAX_GENTITIES,
        );
        let game = game::MoverGame::new(movers.entities, movers.sources, movers.trains, brush_entities, areas.clone());
        if game.published().next().is_some() {
            devprintln!(
                1,
                "LOCAL SERVER MOVERS: {} of {} brush entities spawned as ET_MOVER ({}; {} model2)",
                game.published().count(),
                brush_entities.len(),
                game.summary(),
                movers.models.len()
            );
        }
        let map_models = movers.models;
        let mut server = Self {
            diagnostic_epoch: std::time::Instant::now(),
            player,
            map_name,
            client_name: clean_info_value(&client_info.name),
            worldspawn_music: worldspawn_music.unwrap_or_default().to_owned(),
            configstrings: BTreeMap::new(),
            pending_server_commands: VecDeque::new(),
            server_command_sequence: 0,
            map_effects,
            fx_entities,
            game,
            map_models,
            next_message_num: 1,
            player_time_offset: 0,
            last_snapshot_time: None,
            areas,
            custom_rgba: [255; 4],
        };
        server.rebuild_gamestate(client_info);
        Ok(server)
    }

    pub fn map_name(&self) -> &str {
        &self.map_name
    }

    pub(crate) fn diagnostic_epoch(&self) -> std::time::Instant {
        self.diagnostic_epoch
    }

    /// Server-side `generic_cmd` cases currently owned by this lightweight
    /// authority. Client commands must not be swallowed unless the local game
    /// side can actually execute them.
    pub fn supports_generic_command(&self, command: u8) -> bool {
        matches!(
            command,
            jka_movement::GENCMD_SABERSWITCH | jka_movement::GENCMD_SABERATTACKCYCLE
        )
    }

    pub fn configstrings(&self) -> &BTreeMap<u16, Vec<u8>> {
        &self.configstrings
    }

    pub fn player_configstring(&self) -> Option<&[u8]> {
        self.configstrings.get(&CS_PLAYERS).map(Vec::as_slice)
    }

    /// `char_color_red/green/blue` of the local client, as `ps.customRGBA`.
    pub fn set_char_color(&mut self, rgb: [u8; 3]) {
        self.custom_rgba = [i32::from(rgb[0]), i32::from(rgb[1]), i32::from(rgb[2]), 255];
    }

    pub fn set_client_info(&mut self, info: &ClientInfo) {
        self.client_name = clean_info_value(&info.name);
        self.configstrings
            .insert(CS_PLAYERS, client_info_string(info).into_bytes());
    }

    /// Local equivalent of `SV_SendServerCommand`'s reliable command stream.
    /// `push_local_snapshot` drains this into CGame before the snapshot whose
    /// `server_command_num` acknowledges the sequence.
    pub fn drain_server_commands(&mut self) -> Vec<ServerCommand> {
        self.pending_server_commands.drain(..).collect()
    }

    fn send_server_command(&mut self, text: Vec<u8>) {
        self.server_command_sequence = self.server_command_sequence.saturating_add(1).max(1);
        self.pending_server_commands.push_back(ServerCommand {
            sequence: self.server_command_sequence,
            text,
        });
    }

    pub fn set_saber_movement_info(
        &mut self,
        sabers: [SaberMovementInfo; 2],
    ) -> Result<(), String> {
        self.player.set_saber_movement_info(sabers)
    }

    /// See `LocalPlayer::set_foot_bolts`.
    pub fn set_foot_bolts(&mut self, bolts: Option<[[f32; 3]; 2]>) {
        self.player.set_foot_bolts(bolts);
    }

    fn rebuild_gamestate(&mut self, info: &ClientInfo) {
        self.configstrings.clear();
        self.configstrings.insert(
            CS_SERVERINFO,
            format!(
                "\\mapname\\{}\\g_gametype\\0\\g_maxForceRank\\7\\g_forcePowerDisable\\0\\g_weaponDisable\\0\\g_duelWeaponDisable\\0\\g_forceBasedTeams\\0\\sv_hostname\\DinurdoJK Local",
                clean_info_value(&self.map_name)
            )
            .into_bytes(),
        );
        self.configstrings.insert(
            CS_SYSTEMINFO,
            b"\\sv_serverid\\1\\sv_pure\\0".to_vec(),
        );
        // TaystJK SP_worldspawn publishes even an empty music value.
        self.configstrings.insert(CS_MUSIC, self.worldspawn_music.as_bytes().to_vec());
        for (slot, model) in self.map_models.iter().enumerate() {
            let Ok(slot) = u16::try_from(slot + 1) else { break };
            self.configstrings
                .insert(CS_MODELS + slot, model.as_bytes().to_vec());
        }
        // G_SoundSetIndex: the bmodelSets of movers (door and lift sounds).
        for (slot, set) in self.game.sound_sets().iter().enumerate() {
            let Some(index) = u16::try_from(slot + 1).ok().and_then(|slot| CS_AMBIENT_SET.checked_add(slot)) else { break };
            self.configstrings.insert(index, set.as_bytes().to_vec());
        }
        for (slot, effect) in self.map_effects.iter().enumerate() {
            let Ok(slot) = u16::try_from(slot + 1) else { break };
            self.configstrings
                .insert(CS_EFFECTS + slot, effect.as_bytes().to_vec());
        }
        self.set_client_info(info);
    }

    fn map_player_time(&self, player_time: i32) -> i32 {
        (i64::from(player_time) + self.player_time_offset)
            .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
    }

    fn authoritative_time(&self) -> i32 {
        self.map_player_time(self.player.view().command_time)
    }

    fn rebase_after_player_time_reset(&mut self, previous_server_time: i32) {
        let raw = self.player.view().command_time;
        let next = previous_server_time.saturating_add(1);
        self.player_time_offset = i64::from(next) - i64::from(raw);
    }

    /// Return a fresh authoritative snapshot when Pmove advanced (or when a
    /// caller explicitly needs a state transition such as join/noclip/respawn).
    pub fn take_snapshot(&mut self, force: bool) -> Option<Snapshot> {
        let server_time = self.authoritative_time();
        if !force && self.last_snapshot_time == Some(server_time) {
            return None;
        }

        let network = self.player.network_state();
        let mut player_state = ProtocolPlayerState::default();
        player_state.fields = network.fields;
        player_state.stats = network.stats;
        player_state.persistant = network.persistant;
        player_state.ammo = network.ammo;
        player_state.powerups = network.powerups;
        for (index, value) in self.custom_rgba.into_iter().enumerate() {
            player_state.set_field_bits(&format!("customRGBA[{index}]"), value as u32);
        }

        let message_num = self.next_message_num;
        self.next_message_num = self.next_message_num.saturating_add(1).max(1);
        let delta_num = if message_num <= 1 { -1 } else { message_num - 1 };
        self.last_snapshot_time = Some(server_time);

        Some(Snapshot {
            server_time,
            message_num,
            delta_num,
            snap_flags: 0,
            ping: 0,
            server_command_num: self.server_command_sequence,
            area_mask: self.area_mask(),
            player_state,
            vehicle_player_state: None,
            // The local client is represented by playerState exactly like the
            // predicted/followed client on a remote server. Map-authored entities
            // are published through this same snapshot boundary; CGame does not
            // need a special Solo Game presentation path.
            entities: self
                .fx_entities
                .iter()
                .chain(self.game.published())
                .cloned()
                .collect(),
        })
    }

    /// SV_BuildClientSnapshot: CM_WriteAreaBits for the area holding the
    /// view origin, inverted onto the wire (a set bit hides an area). Every
    /// areaportal starts closed (cm.areaPortals is zeroed at load) and opens
    /// with the door that guards it, so the flood reaches the viewer's own
    /// area plus whatever open portals lead to. A view in solid (area -1)
    /// sees every area, as in CM_WriteAreaBits.
    fn area_mask(&self) -> [u8; 32] {
        let Some(areas) = &self.areas else {
            return [0; 32];
        };
        self.game.area_mask(areas.area_at(self.player.view().eye_origin()))
    }

    pub fn update(
        &mut self,
        elapsed: Duration,
        keys: &HashSet<KeyCode>,
        mouse: (f64, f64),
        client_cmd: jka_movement::UserCmd,
    ) -> Result<Vec<SurfaceDeformationStamp>, String> {
        // Movers run on the snapshot timeline, which player time resets rebase.
        self.game.set_time_offset(self.player_time_offset.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32);
        self.player.update_with_game(elapsed, keys, mouse, client_cmd, &mut self.game)?;
        Ok(self.player.take_deformation_stamps())
    }

    pub fn mode(&self) -> JoinMode {
        self.player.mode
    }

    pub fn view(&self) -> PlayerView {
        self.player.view()
    }

    /// `fd.saberDrawAnimLevel` (playerState field 25 in the protocol-26 schema).
    /// Unlike `fd.saberAnimLevel` this follows a style cycled mid-swing at once,
    /// which is what the cgame HUD shows.
    pub fn saber_style(&self) -> i32 {
        const SABER_DRAW_ANIM_LEVEL_FIELD: usize = 25;
        self.player
            .network_state()
            .fields
            .get(SABER_DRAW_ANIM_LEVEL_FIELD)
            .map_or(0, |bits| *bits as i32)
    }

    /// `fd.saberAnimLevel` (playerState field 23 in the protocol-26 schema):
    /// the saber style which is actually active, as opposed to the HUD-facing
    /// queued style returned by `saber_style()`.
    pub fn saber_active_style(&self) -> i32 {
        const SABER_ANIM_LEVEL_FIELD: usize = 23;
        self.player
            .network_state()
            .fields
            .get(SABER_ANIM_LEVEL_FIELD)
            .map_or(0, |bits| *bits as i32)
    }

    pub fn entity_view(&self) -> PlayerEntityView {
        let mut view = self.player.entity_view();
        view.custom_rgba = self.custom_rgba;
        view
    }

    pub fn presentation_time(&self) -> i32 {
        self.map_player_time(self.player.presentation_time())
    }

    pub fn subframe_view_angles(&self) -> [f32; 3] {
        self.player.subframe_view_angles()
    }

    pub fn camera(&self, camera: &mut Camera) {
        self.player.camera(camera);
    }

    pub fn camera_subframe(&self, camera: &mut Camera) {
        self.player.camera_subframe(camera);
    }

    pub fn world_position(&self) -> glam::Vec3 {
        self.player.world_position()
    }

    pub fn can_join(&self) -> bool {
        self.player.can_join()
    }

    /// OpenJK Cmd_Team_f with no argument: print the current session team.
    pub fn report_current_team(&mut self) {
        self.send_server_command(current_team_command(local_team(self.player.mode)));
    }

    /// Install freshly rebuilt source-map collision while preserving the
    /// authoritative local player and snapshot timeline.
    pub fn replace_collision_world(&mut self, world: CollisionWorld) {
        self.player.replace_collision_world(world);
    }

    pub fn join(&mut self, mode: JoinMode, spawn: SpawnPoint) -> Result<(), String> {
        let old_team = local_team(self.player.mode);
        let new_team = local_team(mode);
        let previous_server_time = self.authoritative_time();
        self.player.join(mode, spawn)?;
        self.rebase_after_player_time_reset(previous_server_time);

        // TaystJK SetTeam returns before BroadcastTeamChange for an unchanged
        // non-spectator team. Spectator->spectator is allowed through SetTeam,
        // but BroadcastTeamChange itself intentionally prints nothing for it.
        if new_team != old_team || new_team == TEAM_SPECTATOR {
            self.broadcast_team_change(old_team, new_team);
        }
        Ok(())
    }

    /// Direct port of TaystJK `BroadcastTeamChange`'s announcement branches.
    ///
    /// The real game module resolves `G_GetStringEdString("MP_SVGAME", key)`
    /// before sending the command. The in-process server intentionally leaves
    /// that one transport detail as an `@@@key` reference so the existing
    /// client-side `CG_CheckSVStringEdRef` path resolves the same StringEd text;
    /// the resulting presentation and command type (`cp` vs `print`) are the
    /// same.
    fn broadcast_team_change(&mut self, old_team: i32, new_team: i32) {
        let Some(text) = broadcast_team_change_command(
            &self.client_name,
            old_team,
            new_team,
            GT_FFA,
            false,
        ) else {
            return;
        };
        self.send_server_command(text);
    }

    pub fn respawn(&mut self, spawn: SpawnPoint) -> Result<(), String> {
        let previous_server_time = self.authoritative_time();
        self.player.respawn(spawn)?;
        self.rebase_after_player_time_reset(previous_server_time);
        Ok(())
    }

    pub fn set_idrive(&mut self, enabled: bool) {
        self.player.set_idrive(enabled);
    }

    pub fn set_mouse_input_settings(&mut self, settings: MouseInputSettings) {
        self.player.set_mouse_input_settings(settings);
    }

    pub fn set_command_rate(&mut self, command_rate: u32) -> Result<(), String> {
        self.player.set_command_rate(command_rate)
    }

    pub fn latest_command_msec(&self) -> i32 {
        self.player.latest_command_msec()
    }

    pub fn pause(&mut self) {
        self.player.pause();
    }

    pub fn request_power(&mut self, power: MovementPower) {
        self.player.request_power(power);
    }

    pub fn note_movement_key_press(&mut self, key: KeyCode) {
        self.player.note_movement_key_press(key);
    }

    pub fn set_noclip(&mut self, enabled: bool) -> Result<bool, String> {
        self.player.set_noclip(enabled)
    }

    pub fn toggle_noclip(&mut self) -> Result<bool, String> {
        self.player.toggle_noclip()
    }

    pub fn give_all(&mut self) -> Result<(), String> {
        self.player.give_all()
    }

    pub fn teleport(&mut self, origin: [f32; 3], angles: [f32; 3]) -> Result<(), String> {
        self.player.teleport(origin, angles)
    }

    pub fn teleport_with_push(&mut self, origin: [f32; 3], angles: [f32; 3], speed: i32) -> Result<(), String> {
        self.player.teleport_with_push(origin, angles, speed)
    }

    /// `/trace` menu's "USE / TOGGLE" action: fires a mover/trigger-logic
    /// entity by its live entity number, as if the player had used it.
    pub fn use_entity(&mut self, entity_num: u16) -> Result<(), String> {
        self.game.use_entity(entity_num)
    }

    pub fn apply_mouse_look_timed(&mut self, mouse: (f64, f64), elapsed: Duration) {
        self.player.apply_mouse_look_timed(mouse, elapsed);
    }

}

impl Deref for LocalServer {
    type Target = LocalPlayer;

    fn deref(&self) -> &Self::Target {
        &self.player
    }
}

impl DerefMut for LocalServer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.player
    }
}

/// The `saberInfo_t` gameplay data OpenJK's `WP_SetSaber` leaves in
/// `clientInfo_t::saber[]` for the two configured saber names. The local
/// authority and networked prediction share this so every Pmove sees the same
/// equipment the server does (two-handed and `notInMP` rules included).
pub fn saber_movement_loadout(
    definitions: &SaberDefinitions,
    saber_names: [&str; 2],
    player_client: bool,
) -> [SaberMovementInfo; 2] {
    fn converted(definition: &SaberDefinition) -> SaberMovementInfo {
        SaberMovementInfo {
            present: 1,
            num_blades: definition.num_blades.clamp(1, 8) as i32,
            styles_learned: definition.styles_learned,
            styles_forbidden: definition.styles_forbidden,
            saber_flags: definition.saber_flags,
            move_speed_scale: definition.move_speed_scale,
            anim_speed_scale: definition.anim_speed_scale,
            ready_anim: definition.ready_anim,
            draw_anim: definition.draw_anim,
            putaway_anim: definition.putaway_anim,
            owns_entity_slot: 1,
            no_manual_deactivate: i32::from(definition.no_manual_deactivate),
            no_manual_deactivate2: i32::from(definition.no_manual_deactivate2),
            blade_style2_start: definition.blade_style2_start.min(8) as i32,
            single_blade_style: definition.single_blade_style,
        }
    }

    definitions
        .equip(saber_names, player_client)
        .map(|slot| slot.as_ref().map_or_else(SaberMovementInfo::default, converted))
}

fn entity_field_index(name: &str) -> Option<usize> {
    ENTITY_FIELDS
        .iter()
        .position(|(candidate, _)| *candidate == name)
}

fn set_entity_i32(entity: &mut EntityState, name: &str, value: i32) {
    if let Some(index) = entity_field_index(name) {
        entity.fields[index] = value as u32;
    }
}

fn set_entity_f32(entity: &mut EntityState, name: &str, value: f32) {
    if let Some(index) = entity_field_index(name) {
        entity.fields[index] = value.to_bits();
    }
}

fn set_entity_vec3(entity: &mut EntityState, prefix: &str, value: [f32; 3]) {
    for (axis, value) in value.into_iter().enumerate() {
        set_entity_f32(entity, &format!("{prefix}[{axis}]"), value);
    }
}

fn build_map_fx_entities(
    runners: &[MapFxRunner],
    registrations: &[MapWorldEffectRegistration],
) -> (Vec<String>, Vec<EntityState>) {
    let mut effects = Vec::<String>::new();
    let mut effect_indices = BTreeMap::<String, i32>::new();
    let mut entities = Vec::new();

    // G_SpawnEntitiesFromString invokes each entity spawn function in map order.
    // Both dedicated weather entities and fx_runner call G_EffectIndex, so merge
    // them by their source BSP entity index before filling CS_EFFECTS. This is
    // observable for commands such as *clear/*die and therefore is not merely
    // bookkeeping.
    enum Registration<'a> {
        Weather(&'a MapWorldEffectRegistration),
        Runner(&'a MapFxRunner),
    }
    let mut ordered = Vec::<(usize, usize, Registration<'_>)>::with_capacity(
        registrations.len() + runners.len(),
    );
    for (sequence, registration) in registrations.iter().enumerate() {
        ordered.push((registration.source_entity_index, sequence, Registration::Weather(registration)));
    }
    let weather_count = registrations.len();
    for (sequence, runner) in runners.iter().enumerate() {
        ordered.push((runner.source_entity_index, weather_count + sequence, Registration::Runner(runner)));
    }
    ordered.sort_by_key(|(entity_index, sequence, _)| (*entity_index, *sequence));

    for (_, _, registration) in ordered {
        let (raw_effect, runner) = match registration {
            Registration::Weather(registration) => (registration.effect.as_str(), None),
            Registration::Runner(runner) => (runner.effect.as_str(), Some(runner)),
        };
        let normalized = raw_effect.replace('\\', "/");
        let key = normalized.to_ascii_lowercase();
        let effect_index = if let Some(&index) = effect_indices.get(&key) {
            index
        } else {
            if effects.len() >= MAX_LOCAL_EFFECTS {
                eprintln!("LOCAL SERVER FX: CS_EFFECTS limit reached; skipping '{raw_effect}'");
                continue;
            }
            let index = (effects.len() + 1) as i32;
            effects.push(normalized.clone());
            effect_indices.insert(key, index);
            index
        };

        let Some(runner) = runner else { continue };

        // Native CGame leaves gameEffects[slot] == 0 when the registered
        // string starts with '*'. The ET_FX therefore has no ordinary .efx to
        // play; the visible result came from CG_ParseWeatherEffect already.
        if normalized.trim_start().starts_with('*') {
            continue;
        }
        if LOCAL_MAP_ENTITY_BASE.saturating_add(entities.len() as u16) >= MAX_GENTITIES {
            eprintln!("LOCAL SERVER FX: entity limit reached; skipping fx_runner '{}'", runner.effect);
            continue;
        }

        let number = LOCAL_MAP_ENTITY_BASE + entities.len() as u16;
        let mut state = EntityState {
            number,
            fields: [0; ENTITY_FIELDS.len()],
        };
        set_entity_i32(&mut state, "eType", ET_FX);
        set_entity_i32(&mut state, "pos.trType", 0); // TR_STATIONARY
        set_entity_vec3(&mut state, "pos.trBase", runner.origin);
        set_entity_vec3(&mut state, "origin", runner.origin);
        set_entity_i32(&mut state, "apos.trType", 0); // TR_STATIONARY
        set_entity_vec3(&mut state, "apos.trBase", runner.angles);
        set_entity_vec3(&mut state, "angles", runner.angles);
        set_entity_i32(&mut state, "modelindex", effect_index);
        set_entity_i32(
            &mut state,
            "modelindex2",
            if runner.spawnflags & (FX_RUNNER_STARTOFF | FX_RUNNER_ONESHOT) != 0 {
                FX_STATE_OFF
            } else {
                FX_STATE_CONTINUOUS
            },
        );
        set_entity_f32(&mut state, "speed", runner.delay_ms as f32);
        set_entity_i32(&mut state, "time", runner.random_ms);
        entities.push(state);
    }

    if !effects.is_empty() {
        devprintln!(
            1,
            "LOCAL SERVER FX: {} fx_runner entity(s), {} unique CS_EFFECTS registration(s)",
            entities.len(),
            effects.len()
        );
    }
    (effects, entities)
}

fn normalize_map_name(name: &str) -> String {
    let normalized = name.replace('\\', "/");
    normalized
        .strip_suffix(".bsp")
        .or_else(|| normalized.strip_suffix(".map"))
        .unwrap_or(&normalized)
        .to_owned()
}

fn clean_info_value(value: &str) -> String {
    value
        .chars()
        .filter(|ch| *ch != '\\' && *ch != '"' && *ch != '\n' && *ch != '\r')
        .collect()
}

fn local_team(mode: JoinMode) -> i32 {
    match mode {
        JoinMode::Player => TEAM_FREE,
        JoinMode::Spectator => TEAM_SPECTATOR,
    }
}

fn current_team_command(team: i32) -> Vec<u8> {
    let key = match team {
        TEAM_BLUE => "PRINTBLUETEAM",
        TEAM_RED => "PRINTREDTEAM",
        TEAM_SPECTATOR => "PRINTSPECTEAM",
        _ => "PRINTFREETEAM",
    };
    format!("print \"@@@{key}\n\"").into_bytes()
}

/// TaystJK `BroadcastTeamChange` reduced to the reliable command it emits.
/// Keeping all four team branches here makes the local authority's behavior
/// explicit even though today's one-client solo host only exposes FREE/SPEC.
fn broadcast_team_change_command(
    client_name: &str,
    old_team: i32,
    new_team: i32,
    gametype: i32,
    race_mode: bool,
) -> Option<Vec<u8>> {
    if gametype == GT_SIEGE {
        return None;
    }

    let stringed_key = if new_team == TEAM_RED {
        "JOINEDTHEREDTEAM"
    } else if new_team == TEAM_BLUE {
        "JOINEDTHEBLUETEAM"
    } else if new_team == TEAM_SPECTATOR && old_team != TEAM_SPECTATOR {
        "JOINEDTHESPECTATORS"
    } else if new_team == TEAM_FREE {
        if gametype == GT_DUEL || gametype == GT_POWERDUEL {
            return None;
        }
        "JOINEDTHEBATTLE"
    } else {
        return None;
    };

    // jaPRO/Tayst race mode deliberately demotes these announcements from
    // centreprint to ordinary console print.
    let command = if race_mode { "print" } else { "cp" };
    Some(format!("{command} \"{client_name}^7 @@@{stringed_key}\n\"").into_bytes())
}

fn client_info_string(info: &ClientInfo) -> String {
    format!(
        "\\n\\{}\\t\\{}\\ds\\{}\\model\\{}\\c1\\{}\\c2\\{}\\st\\{}\\st2\\{}",
        clean_info_value(&info.name),
        info.team,
        if info.female { "f" } else { "m" },
        clean_info_value(&info.model_cvar()),
        info.saber_color,
        info.saber2_color,
        clean_info_value(&info.saber_name),
        clean_info_value(&info.saber2_name),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_client_info_uses_protocol_info_keys() {
        let info = ClientInfo::solo_model("jedi_hm/head_a1|torso_d1|lower_a1");
        let text = client_info_string(&info);
        assert!(text.contains("\\model\\jedi_hm/head_a1|torso_d1|lower_a1"));
        assert!(text.contains("\\c1\\4"));
    }

    #[test]
    fn local_server_publishes_worldspawn_music_as_cs_music() {
        let info = ClientInfo::solo_model("kyle/default");
        let create = |music: Option<&str>| {
            LocalServer::new(
                "mp/ffa3", None, None, SpawnPoint::default(), &info,
                [SaberMovementInfo::equipped_default(), SaberMovementInfo::default()],
                &[], &[], &[], music, None,
            ).expect("local spectator server")
        };
        assert_eq!(
            create(Some("music/intro.mp3 music/loop.mp3")).configstrings().get(&CS_MUSIC),
            Some(&b"music/intro.mp3 music/loop.mp3".to_vec()),
        );
        assert_eq!(create(None).configstrings().get(&CS_MUSIC), Some(&Vec::new()));
    }

    #[test]
    fn map_name_is_protocol_style() {
        assert_eq!(normalize_map_name("mp/ffa3.bsp"), "mp/ffa3");
        assert_eq!(normalize_map_name("mp\\ffa3"), "mp/ffa3");
    }

    #[test]
    fn tayst_broadcast_team_change_uses_cp_and_stringed_keys() {
        assert_eq!(
            broadcast_team_change_command("Padawan", TEAM_SPECTATOR, TEAM_FREE, GT_FFA, false),
            Some(b"cp \"Padawan^7 @@@JOINEDTHEBATTLE\n\"".to_vec())
        );
        assert_eq!(
            broadcast_team_change_command("Padawan", TEAM_FREE, TEAM_SPECTATOR, GT_FFA, false),
            Some(b"cp \"Padawan^7 @@@JOINEDTHESPECTATORS\n\"".to_vec())
        );
        assert_eq!(
            broadcast_team_change_command("Padawan", TEAM_SPECTATOR, TEAM_RED, GT_TEAM, false),
            Some(b"cp \"Padawan^7 @@@JOINEDTHEREDTEAM\n\"".to_vec())
        );
        assert_eq!(
            broadcast_team_change_command("Padawan", TEAM_RED, TEAM_BLUE, GT_TEAM, false),
            Some(b"cp \"Padawan^7 @@@JOINEDTHEBLUETEAM\n\"".to_vec())
        );
    }

    #[test]
    fn tayst_broadcast_team_change_preserves_suppression_and_race_prints() {
        assert_eq!(
            broadcast_team_change_command(
                "Padawan",
                TEAM_SPECTATOR,
                TEAM_SPECTATOR,
                GT_FFA,
                false,
            ),
            None
        );
        assert_eq!(
            broadcast_team_change_command("Padawan", TEAM_SPECTATOR, TEAM_FREE, GT_DUEL, false),
            None
        );
        assert_eq!(
            broadcast_team_change_command("Padawan", TEAM_FREE, TEAM_RED, GT_SIEGE, false),
            None
        );
        assert_eq!(
            broadcast_team_change_command("Padawan", TEAM_FREE, TEAM_SPECTATOR, GT_FFA, true),
            Some(b"print \"Padawan^7 @@@JOINEDTHESPECTATORS\n\"".to_vec())
        );
    }

    #[test]
    fn openjk_bare_team_reports_current_free_or_spectator_team() {
        assert_eq!(current_team_command(TEAM_FREE), b"print \"@@@PRINTFREETEAM\n\"".to_vec());
        assert_eq!(
            current_team_command(TEAM_SPECTATOR),
            b"print \"@@@PRINTSPECTEAM\n\"".to_vec()
        );
    }

    #[test]
    fn local_fx_runner_uses_effect_configstring_and_et_fx() {
        let runner = MapFxRunner {
                bake_static: false,
            source_entity_index: 0,
            effect: "effects/test.efx".into(),
            origin: [1.0, 2.0, 3.0],
            angles: [-90.0, 0.0, 0.0],
            delay_ms: 200,
            random_ms: 25,
            spawnflags: 0,
        };
        let (effects, entities) = build_map_fx_entities(&[runner], &[]);
        assert_eq!(effects, vec!["effects/test.efx".to_owned()]);
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].field_i32("eType"), Some(ET_FX));
        assert_eq!(entities[0].field_i32("modelindex"), Some(1));
        assert_eq!(entities[0].field_i32("modelindex2"), Some(FX_STATE_CONTINUOUS));
        assert_eq!(entities[0].field_f32("pos.trBase[0]"), Some(1.0));
    }

    #[test]
    fn map_world_effect_registration_uses_cs_effects_without_et_fx() {
        let registrations = vec![
            MapWorldEffectRegistration { source_entity_index: 0, effect: "*rain".to_owned() },
            MapWorldEffectRegistration { source_entity_index: 0, effect: "*fog".to_owned() },
        ];
        let (effects, entities) = build_map_fx_entities(&[], &registrations);
        assert_eq!(effects, vec!["*rain".to_owned(), "*fog".to_owned()]);
        assert!(entities.is_empty());
    }

    #[test]
    fn map_weather_and_star_fx_runner_preserve_source_entity_order() {
        let registrations = vec![MapWorldEffectRegistration {
            source_entity_index: 2,
            effect: "*rain".to_owned(),
        }];
        let runner = MapFxRunner {
                bake_static: false,
            source_entity_index: 1,
            effect: "*clear".into(),
            origin: [0.0; 3],
            angles: [0.0; 3],
            delay_ms: 200,
            random_ms: 0,
            spawnflags: 0,
        };
        let (effects, entities) = build_map_fx_entities(&[runner], &registrations);
        assert_eq!(effects, vec!["*clear".to_owned(), "*rain".to_owned()]);
        assert!(entities.is_empty());
    }

    #[test]
    fn star_fx_runner_registers_weather_but_does_not_spawn_et_fx() {
        let runner = MapFxRunner {
                bake_static: false,
            source_entity_index: 0,
            effect: "*heavyrain".into(),
            origin: [1.0, 2.0, 3.0],
            angles: [0.0; 3],
            delay_ms: 200,
            random_ms: 0,
            spawnflags: 0,
        };
        let (effects, entities) = build_map_fx_entities(&[runner], &[]);
        assert_eq!(effects, vec!["*heavyrain".to_owned()]);
        assert!(entities.is_empty());
    }

    #[test]
    fn startoff_and_oneshot_fx_runners_begin_off() {
        for spawnflags in [FX_RUNNER_STARTOFF, FX_RUNNER_ONESHOT] {
            let runner = MapFxRunner {
                bake_static: false,
                source_entity_index: 0,
                effect: "test".into(),
                origin: [0.0; 3],
                angles: [-90.0, 0.0, 0.0],
                delay_ms: 200,
                random_ms: 0,
                spawnflags,
            };
            let (_, entities) = build_map_fx_entities(&[runner], &[]);
            assert_eq!(entities[0].field_i32("modelindex2"), Some(FX_STATE_OFF));
        }
    }
}

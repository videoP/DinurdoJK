//! Lightweight in-process server shim for `/map` and `/devmap`.
//!
//! The local authority still reuses the existing vendored-OpenJK Pmove host,
//! but it no longer owns a separate rendering path.  Instead it publishes the
//! same decoded protocol objects (`configstrings` + `Snapshot`) that a remote
//! server produces after netchan decoding.  CGame/presentation therefore sees
//! local and remote play through the same boundary.

use std::{
    collections::{BTreeMap, HashSet},
    ops::{Deref, DerefMut},
    time::Duration,
};

use winit::keyboard::KeyCode;

use jka_assets::saber::{SaberDefinition, SaberDefinitions};

use crate::{
    camera::Camera,
    cgame::{ClientInfo, CS_EFFECTS, CS_PLAYERS, CS_SERVERINFO, ET_FX},
    player::{LocalPlayer, MouseInputSettings},
    scene::{MapFxRunner, SpawnPoint},
    surface_deformation::SurfaceDeformationStamp,
};
use jka_movement::{
    CollisionWorld, JoinMode, MovementPower, PlayerEntityView, PlayerView, PmoveContext,
    SaberMovementInfo,
};
use jka_protocol::{
    gamestate::{EntityState, ENTITY_FIELDS},
    server::{PlayerState as ProtocolPlayerState, Snapshot},
    session::CS_SYSTEMINFO,
};

const FX_STATE_OFF: i32 = 0;
const FX_STATE_CONTINUOUS: i32 = 20;
const FX_RUNNER_STARTOFF: i32 = 1;
const FX_RUNNER_ONESHOT: i32 = 2;
const MAX_LOCAL_EFFECTS: usize = 63;
const LOCAL_MAP_ENTITY_BASE: u16 = 64;
const MAX_GENTITIES: u16 = 1024;

/// A one-client authoritative local host.  There is deliberately no fake UDP
/// socket or protocol-26 re-encoding here: this sits immediately after the
/// network decoder and feeds the exact semantic objects consumed by CGame.
pub struct LocalServer {
    player: LocalPlayer,
    map_name: String,
    configstrings: BTreeMap<u16, Vec<u8>>,
    map_effects: Vec<String>,
    map_entities: Vec<EntityState>,
    next_message_num: i32,
    /// Offset maps Pmove's commandTime into a monotonic server timeline. Pmove
    /// player states legitimately reset commandTime on join/respawn; snapshot
    /// serverTime must never run backwards across those state transitions.
    player_time_offset: i64,
    last_snapshot_time: Option<i32>,
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
    ) -> Result<Self, String> {
        let mut player = LocalPlayer::new(movement, world, spawn)?;
        player.set_saber_movement_info(saber_movement)?;
        let map_name = normalize_map_name(map_name);
        let (map_effects, map_entities) = build_map_fx_entities(fx_runners);
        let mut server = Self {
            player,
            map_name,
            configstrings: BTreeMap::new(),
            map_effects,
            map_entities,
            next_message_num: 1,
            player_time_offset: 0,
            last_snapshot_time: None,
        };
        server.rebuild_gamestate(client_info);
        Ok(server)
    }

    pub fn map_name(&self) -> &str {
        &self.map_name
    }

    /// Server-side `generic_cmd` cases currently owned by this lightweight
    /// authority. Client commands must not be swallowed unless the local game
    /// side can actually execute them.
    pub fn supports_generic_command(&self, command: u8) -> bool {
        matches!(command, jka_movement::GENCMD_SABERATTACKCYCLE)
    }

    pub fn configstrings(&self) -> &BTreeMap<u16, Vec<u8>> {
        &self.configstrings
    }

    pub fn player_configstring(&self) -> Option<&[u8]> {
        self.configstrings.get(&CS_PLAYERS).map(Vec::as_slice)
    }

    pub fn set_client_info(&mut self, info: &ClientInfo) {
        self.configstrings
            .insert(CS_PLAYERS, client_info_string(info).into_bytes());
    }

    pub fn set_saber_movement_info(
        &mut self,
        sabers: [SaberMovementInfo; 2],
    ) -> Result<(), String> {
        self.player.set_saber_movement_info(sabers)
    }

    fn rebuild_gamestate(&mut self, info: &ClientInfo) {
        self.configstrings.clear();
        self.configstrings.insert(
            CS_SERVERINFO,
            format!(
                "\\mapname\\{}\\g_gametype\\0\\sv_hostname\\DinurdoJK Local",
                clean_info_value(&self.map_name)
            )
            .into_bytes(),
        );
        self.configstrings.insert(
            CS_SYSTEMINFO,
            b"\\sv_serverid\\1\\sv_pure\\0".to_vec(),
        );
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

        let message_num = self.next_message_num;
        self.next_message_num = self.next_message_num.saturating_add(1).max(1);
        let delta_num = if message_num <= 1 { -1 } else { message_num - 1 };
        self.last_snapshot_time = Some(server_time);

        Some(Snapshot {
            server_time,
            message_num,
            delta_num,
            snap_flags: 0,
            server_command_num: 0,
            area_mask: [0; 32],
            player_state,
            vehicle_player_state: None,
            // The local client is represented by playerState exactly like the
            // predicted/followed client on a remote server. Map-authored entities
            // are published through this same snapshot boundary; CGame does not
            // need a special Solo Game presentation path.
            entities: self.map_entities.clone(),
        })
    }

    pub fn update(
        &mut self,
        elapsed: Duration,
        keys: &HashSet<KeyCode>,
        mouse: (f64, f64),
        client_cmd: jka_movement::UserCmd,
    ) -> Result<Vec<SurfaceDeformationStamp>, String> {
        self.player.update(elapsed, keys, mouse, client_cmd)?;
        Ok(self.player.take_deformation_stamps())
    }

    pub fn mode(&self) -> JoinMode {
        self.player.mode
    }

    pub fn view(&self) -> PlayerView {
        self.player.view()
    }

    pub fn entity_view(&self) -> PlayerEntityView {
        self.player.entity_view()
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

    /// Install freshly rebuilt source-map collision while preserving the
    /// authoritative local player and snapshot timeline.
    pub fn replace_collision_world(&mut self, world: CollisionWorld) {
        self.player.replace_collision_world(world);
    }

    pub fn join(&mut self, mode: JoinMode, spawn: SpawnPoint) -> Result<(), String> {
        let previous_server_time = self.authoritative_time();
        self.player.join(mode, spawn)?;
        self.rebase_after_player_time_reset(previous_server_time);
        Ok(())
    }

    pub fn respawn(&mut self, spawn: SpawnPoint) -> Result<(), String> {
        let previous_server_time = self.authoritative_time();
        self.player.respawn(spawn)?;
        self.rebase_after_player_time_reset(previous_server_time);
        Ok(())
    }

    pub fn set_mouse_input_settings(&mut self, settings: MouseInputSettings) {
        self.player.set_mouse_input_settings(settings);
    }

    pub fn set_physics_tick_msec(&mut self, tick_msec: u32) -> Result<(), String> {
        self.player.set_physics_tick_msec(tick_msec)
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

    pub fn toggle_noclip(&mut self) -> Result<bool, String> {
        self.player.toggle_noclip()
    }

    pub fn apply_mouse_look_timed(&mut self, mouse: (f64, f64), elapsed: Duration) {
        self.player.apply_mouse_look_timed(mouse, elapsed);
    }

    pub fn autofocus_distance(&mut self, camera: &Camera, max_distance: f32) -> f32 {
        self.player.autofocus_distance(camera, max_distance)
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

pub fn saber_movement_loadout(
    definitions: &SaberDefinitions,
    info: &ClientInfo,
) -> [SaberMovementInfo; 2] {
    fn converted(definition: SaberDefinition, present: bool) -> SaberMovementInfo {
        SaberMovementInfo {
            present: i32::from(present),
            num_blades: definition.num_blades.clamp(1, 8) as i32,
            styles_learned: definition.styles_learned,
            styles_forbidden: definition.styles_forbidden,
            saber_flags: definition.saber_flags,
            move_speed_scale: definition.move_speed_scale,
            anim_speed_scale: definition.anim_speed_scale,
            ready_anim: definition.ready_anim,
            draw_anim: definition.draw_anim,
            putaway_anim: definition.putaway_anim,
        }
    }

    let primary_removed = info.saber_name.eq_ignore_ascii_case("none")
        || info.saber_name.eq_ignore_ascii_case("remove");
    let primary = definitions.definition_or_default(&info.saber_name);
    let secondary_present = !info.saber2_name.is_empty()
        && !info.saber2_name.eq_ignore_ascii_case("none")
        && !info.saber2_name.eq_ignore_ascii_case("remove");
    let secondary = definitions.definition_or_default(&info.saber2_name);
    [
        converted(primary, !primary_removed),
        converted(secondary, secondary_present),
    ]
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

fn build_map_fx_entities(runners: &[MapFxRunner]) -> (Vec<String>, Vec<EntityState>) {
    let mut effects = Vec::<String>::new();
    let mut effect_indices = BTreeMap::<String, i32>::new();
    let mut entities = Vec::new();

    for runner in runners {
        if LOCAL_MAP_ENTITY_BASE.saturating_add(entities.len() as u16) >= MAX_GENTITIES {
            eprintln!(
                "LOCAL SERVER FX: entity limit reached; skipped {} remaining fx_runner(s)",
                runners.len().saturating_sub(entities.len())
            );
            break;
        }

        let key = runner.effect.replace('\\', "/").to_ascii_lowercase();
        let effect_index = if let Some(&index) = effect_indices.get(&key) {
            index
        } else {
            if effects.len() >= MAX_LOCAL_EFFECTS {
                eprintln!(
                    "LOCAL SERVER FX: CS_EFFECTS limit reached; skipping fx_runner '{}'",
                    runner.effect
                );
                continue;
            }
            let index = (effects.len() + 1) as i32;
            effects.push(runner.effect.replace('\\', "/"));
            effect_indices.insert(key, index);
            index
        };

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

    if !entities.is_empty() {
        println!(
            "LOCAL SERVER FX: {} fx_runner entity(s), {} unique effect(s)",
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

fn client_info_string(info: &ClientInfo) -> String {
    format!(
        "\\n\\{}\\t\\{}\\model\\{}\\c1\\{}\\c2\\{}\\st\\{}\\st2\\{}",
        clean_info_value(&info.name),
        info.team,
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
    fn map_name_is_protocol_style() {
        assert_eq!(normalize_map_name("mp/ffa3.bsp"), "mp/ffa3");
        assert_eq!(normalize_map_name("mp\\ffa3"), "mp/ffa3");
    }

    #[test]
    fn local_fx_runner_uses_effect_configstring_and_et_fx() {
        let runner = MapFxRunner {
            effect: "effects/test.efx".into(),
            origin: [1.0, 2.0, 3.0],
            angles: [-90.0, 0.0, 0.0],
            delay_ms: 200,
            random_ms: 25,
            spawnflags: 0,
        };
        let (effects, entities) = build_map_fx_entities(&[runner]);
        assert_eq!(effects, vec!["effects/test.efx".to_owned()]);
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].field_i32("eType"), Some(ET_FX));
        assert_eq!(entities[0].field_i32("modelindex"), Some(1));
        assert_eq!(entities[0].field_i32("modelindex2"), Some(FX_STATE_CONTINUOUS));
        assert_eq!(entities[0].field_f32("pos.trBase[0]"), Some(1.0));
    }

    #[test]
    fn startoff_and_oneshot_fx_runners_begin_off() {
        for spawnflags in [FX_RUNNER_STARTOFF, FX_RUNNER_ONESHOT] {
            let runner = MapFxRunner {
                effect: "test".into(),
                origin: [0.0; 3],
                angles: [-90.0, 0.0, 0.0],
                delay_ms: 200,
                random_ms: 0,
                spawnflags,
            };
            let (_, entities) = build_map_fx_entities(&[runner]);
            assert_eq!(entities[0].field_i32("modelindex2"), Some(FX_STATE_OFF));
        }
    }
}

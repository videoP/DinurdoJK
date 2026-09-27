//! CG_EntityEvent sound semantics, separated from the device/mixer implementation.
use crate::audio::{AudioBackend, AudioInfo, Listener, LoopRequest, SoundAssets, SoundOrigin, SoundRequest};
use crate::steam_audio::SteamAudioBakeData;
use jka_assets::{
    bsp::AcousticMesh,
    pk3::AssetSearchPath,
    saber::{load_saber_definitions, SaberDefinitions},
    siege::SiegeClassVisual,
};
use std::{collections::{HashMap, HashSet}, sync::Arc};
use super::{
    event_presenter::EventDispatchResult, player_presenter::saber_name_is_removed, ClientGameState,
    PresentationEvent, PresentedEntity, EF_DEAD, ET_NPC, ET_PLAYER,
};

const MAX_CLIENTS: u16 = 32;
const SOLID_BMODEL: i32 = 0x00ff_ffff;
const WP_SABER: i32 = 3;
const MAX_SOUND_CONFIG_BYTES: usize = 4096;

#[derive(Clone, Debug)]
struct CustomSoundProfile {
    sound_dir: String,
    female: bool,
}

pub struct SoundPresenter {
    assets: SoundAssets,
    backend: Option<AudioBackend>,
    warnings: HashSet<String>,
    custom_sound_profiles: HashMap<String, CustomSoundProfile>,
    saber_definitions: SaberDefinitions,
    inline_model_midpoints: Arc<[[f32; 3]]>,
}

impl SoundPresenter {
    pub fn new(mut assets: AssetSearchPath, steam_audio_enabled: bool, steam_audio_binaural_enabled: bool, steam_audio_environmental_enabled: bool) -> Self {
        let saber_definitions = load_saber_definitions(&mut assets).unwrap_or_else(|error| {
            eprintln!("AUDIO SABER DEFINITIONS UNAVAILABLE (default hum only): {error}");
            SaberDefinitions::default()
        });
        let backend = match AudioBackend::open(steam_audio_enabled, steam_audio_binaural_enabled, steam_audio_environmental_enabled) {
            Ok(backend) => {
                let info = backend.info();
                println!(
                    "AUDIO READY: Rodio/CPAL {}ch {}Hz {} buffer={} | WAV+MP3 | 32 voices | output limiter",
                    info.channels, info.sample_rate, info.sample_format, info.buffer_size
                );
                Some(backend)
            }
            Err(error) => { eprintln!("AUDIO DISABLED: {error}"); None }
        };
        Self {
            assets: SoundAssets::new(assets),
            backend,
            warnings: HashSet::new(),
            custom_sound_profiles: HashMap::new(),
            saber_definitions,
            inline_model_midpoints: Arc::from(Vec::new()),
        }
    }

    /// OpenJK `cgs.inlineModelMidpoints` for the loaded BSP.
    pub fn set_inline_model_midpoints(&mut self, midpoints: Arc<[[f32; 3]]>) {
        self.inline_model_midpoints = midpoints;
    }

    /// This frame's looping sounds, as CGame submits them from CG_AddCEntity:
    /// the CG_EntityEffects `loopSound` pass for every entity plus CG_Player's
    /// saber hum. The backend merges and spatializes them per sfx.
    pub fn update_loops(
        &mut self,
        game: &ClientGameState,
        siege_classes: &[SiegeClassVisual],
        entities: &[PresentedEntity],
        followed_entity: Option<&PresentedEntity>,
        listener_entity: u16,
    ) {
        if self.backend.is_none() { return; }
        let mut requests = Vec::new();
        for entity in audio_entities(entities, followed_entity) {
            if let Some(request) = entity_loop(entity, game, &self.inline_model_midpoints) {
                requests.push(request);
            }
            if entity.entity_type == ET_PLAYER {
                saber_hum_loops(entity, game, siege_classes, &self.saber_definitions, listener_entity, &mut requests);
            }
        }
        let mut resolved = Vec::with_capacity(requests.len());
        for request in requests {
            match self.assets.register(&request.qpath) {
                Ok(sound) => resolved.push((request, sound)),
                Err(error) => self.warn_once(&request.qpath, &error),
            }
        }
        if let Some(backend) = &mut self.backend {
            backend.set_loops(resolved);
        }
    }

    /// An FX `sound` primitive: S_StartSound(origin, ENTITYNUM_NONE, CHAN_AUTO).
    pub fn play_fx_sound(&mut self, qpath: &str, origin: [f32; 3]) {
        if self.backend.is_none() {
            return;
        }
        match self.assets.register(qpath) {
            Ok(sound) => {
                if let Some(backend) = &mut self.backend {
                    backend.play(
                        SoundRequest { qpath: qpath.to_owned(), entity: 1023, channel: 0, origin: SoundOrigin::Fixed(origin) },
                        sound,
                    );
                }
            }
            Err(error) => self.warn_once(qpath, &error),
        }
    }

    fn warn_once(&mut self, qpath: &str, error: &str) {
        if self.warnings.len() < 4096 && self.warnings.insert(qpath.to_owned()) {
            eprintln!("AUDIO ASSET WARNING: {qpath}: {error}");
        }
    }

    pub fn frame(
        &mut self,
        listener: Listener,
        entities: &[PresentedEntity],
        followed_entity: Option<&PresentedEntity>,
    ) {
        if let Some(backend) = &mut self.backend {
            backend.frame(
                listener,
                audio_entities(entities, followed_entity)
                    .map(|entity| (entity.number, entity.origin)),
            );
        }
    }
    pub fn set_rate(&mut self, rate: f32) { if let Some(b) = &mut self.backend { b.set_rate(rate); } }
    pub fn clear(&mut self) { if let Some(b) = &mut self.backend { b.clear(); } }
    pub fn set_mix(&mut self, effects_volume: f32, voice_volume: f32, separation: f32) {
        if let Some(backend) = &mut self.backend {
            backend.set_mix(effects_volume, voice_volume, separation);
        }
    }
    pub fn set_steam_audio_enabled(&mut self, enabled: bool) {
        if let Some(backend) = &mut self.backend {
            backend.set_steam_audio_enabled(enabled);
        }
    }

    pub fn set_steam_audio_binaural_enabled(&mut self, enabled: bool) {
        if let Some(backend) = &mut self.backend {
            backend.set_steam_audio_binaural_enabled(enabled);
        }
    }

    pub fn set_steam_audio_environmental_enabled(&mut self, enabled: bool) {
        if let Some(backend) = &mut self.backend {
            backend.set_steam_audio_environmental_enabled(enabled);
        }
    }
    pub fn set_steam_audio_map(
        &mut self,
        mesh: Option<Arc<AcousticMesh>>,
        bake: Option<Arc<SteamAudioBakeData>>,
    ) {
        if let Some(backend) = &mut self.backend {
            backend.set_steam_audio_map(mesh, bake);
        }
    }
    pub fn info(&self) -> Option<AudioInfo> { self.backend.as_ref().map(AudioBackend::info) }
    pub fn source_rate_summary(&self) -> String { self.assets.sample_rate_summary() }

    pub fn dispatch(
        &mut self,
        event: &PresentationEvent,
        game: &ClientGameState,
        siege_classes: &[SiegeClassVisual],
        now: i32,
    ) -> Option<EventDispatchResult> {
        let mut request = match if event.event == 29 {
            self.saber_attack_request(event, game, siege_classes).map(Some)
        } else {
            sound_request(event, game)
        } {
            Ok(Some(request)) => request,
            Ok(None) => return None,
            Err(reason) => return Some(EventDispatchResult::Partial(reason)),
        };
        if self.backend.is_none() {
            return Some(EventDispatchResult::Partial("AUDIO_DEVICE_UNAVAILABLE"));
        }
        // Avoid a burst of old sounds after a stalled frame or fast-forward.
        if now.saturating_sub(event.server_time) > 250 {
            return Some(EventDispatchResult::Partial("SOUND_SKIPPED_CATCHUP"));
        }

        let registered = if request.qpath.starts_with('*') {
            match self.register_custom_sound(game, siege_classes, request.entity, &request.qpath) {
                Ok((resolved_qpath, sound)) => {
                    request.qpath = resolved_qpath;
                    Ok(sound)
                }
                Err(error) => Err(error),
            }
        } else {
            self.assets.register(&request.qpath)
        };

        match registered {
            Ok(sound) => {
                // OpenJK DoFall plays both fallsplat and the player's custom
                // *land1 voice for hard landings. EV_ROLL likewise layers the
                // custom jump exertion over roll1.wav. Keep those two-source
                // cases instead of collapsing CG_EntityEvent to one sound.
                let companion = match event.event {
                    11 if event.parm > 44 => Some(SoundRequest {
                        qpath: "*land1.wav".to_owned(),
                        entity: request.entity,
                        channel: 3, // CHAN_VOICE
                        origin: SoundOrigin::Entity(event.position),
                    }),
                    17 => Some(SoundRequest {
                        qpath: "*jump1.wav".to_owned(),
                        entity: request.entity,
                        channel: 3, // CHAN_VOICE
                        origin: SoundOrigin::Entity(event.position),
                    }),
                    _ => None,
                };
                let companion = companion.and_then(|mut extra| {
                    let registered = if extra.qpath.starts_with('*') {
                        match self.register_custom_sound(game, siege_classes, extra.entity, &extra.qpath) {
                            Ok((resolved_qpath, sound)) => {
                                extra.qpath = resolved_qpath;
                                Some(sound)
                            }
                            Err(error) => {
                                self.warn_once(&extra.qpath, &error);
                                None
                            }
                        }
                    } else {
                        match self.assets.register(&extra.qpath) {
                            Ok(sound) => Some(sound),
                            Err(error) => {
                                self.warn_once(&extra.qpath, &error);
                                None
                            }
                        }
                    };
                    registered.map(|sound| (extra, sound))
                });

                let radio_request = if event.event == 75 {
                    let local_client = game
                        .current_snapshot()
                        .and_then(|snapshot| snapshot.player_state.field_i32("clientNum"))
                        .and_then(|value| u16::try_from(value).ok());
                    let source_info = game.client_info(usize::from(request.entity), siege_classes);
                    let local_info = local_client.and_then(|client| game.client_info(usize::from(client), siege_classes));
                    match (local_client, source_info, local_info) {
                        (Some(local), Some(source), Some(listener))
                            if local != request.entity && source.team == listener.team =>
                        {
                            Some(SoundRequest {
                                qpath: request.qpath.clone(),
                                entity: local,
                                channel: 11, // CHAN_MENU1: radio/head copy
                                origin: SoundOrigin::Local,
                            })
                        }
                        _ => None,
                    }
                } else {
                    None
                };

                if let Some(backend) = &mut self.backend {
                    if let Some(radio) = radio_request {
                        backend.play(radio, Arc::clone(&sound));
                    }
                    backend.play(request, sound);
                    if let Some((extra, extra_sound)) = companion {
                        backend.play(extra, extra_sound);
                    }
                }
                // Absorb-hit also needs a visual team-power effect. Voice commands
                // now have both the in-world CHAN_VOICE and teammate radio copy;
                // their localized chat caption is a separate UI feature.
                Some(if event.event == 40 && event.parm == 3 {
                    EventDispatchResult::Partial("SOUND_PLAYED_FX_PENDING")
                } else if event.event == 75 {
                    EventDispatchResult::Partial("VOICE_SOUND_PLAYED_CHAT_TEXT_PENDING")
                } else {
                    EventDispatchResult::Handled("SOUND_PLAYED")
                })
            }
            Err(error) => {
                self.warn_once(&request.qpath, &error);
                Some(EventDispatchResult::Partial("SOUND_ASSET_UNAVAILABLE"))
            }
        }
    }

    /// OpenJK EV_SABER_ATTACK: use saber[0].swingSound1..3 when authored,
    /// otherwise one of the stock saberhup1..8 sounds on CHAN_WEAPON.
    fn saber_attack_request(
        &self,
        event: &PresentationEvent,
        game: &ClientGameState,
        siege_classes: &[SiegeClassVisual],
    ) -> Result<SoundRequest, &'static str> {
        let info = if event.state.field_i32("eType").unwrap_or(ET_PLAYER) == ET_NPC {
            game.npc_client_info(&event.state).ok()
        } else {
            game.client_info(usize::from(event.entity_num), siege_classes)
        };
        let variant3 = event_variant(event, 3);
        let qpath = info
            .as_ref()
            .and_then(|info| self.saber_definitions.get(&info.saber_name))
            .filter(|definition| definition.swing_sounds[0].is_some())
            .and_then(|definition| {
                definition.swing_sounds[variant3]
                    .clone()
                    .or_else(|| definition.swing_sounds[0].clone())
            })
            .unwrap_or_else(|| {
                format!(
                    "sound/weapons/saber/saberhup{}.wav",
                    event_variant(event, 8) + 1
                )
            });
        let origin = super::entity_vec3(&event.state, "pos.trBase")
            .map(SoundOrigin::Fixed)
            .unwrap_or(SoundOrigin::Fixed(event.position));
        Ok(SoundRequest { qpath, entity: event.entity_num, channel: 2, origin }) // CHAN_WEAPON
    }

    /// OpenJK CG_CustomSound/CG_LoadCISounds behavior for protocol sounds whose
    /// CS_SOUNDS entry begins with '*'. The event carries the client/NPC entity;
    /// that entity's model/skin selects models/players/<model>/sounds*.cfg,
    /// then the named voice resolves under sound/chars/<soundpath>/misc.
    fn register_custom_sound(
        &mut self,
        game: &ClientGameState,
        siege_classes: &[SiegeClassVisual],
        entity: u16,
        custom_name: &str,
    ) -> Result<(String, Arc<crate::audio::RegisteredSound>), String> {
        let info = game
            .entity_state(entity)
            .and_then(|state| {
                if state.field_i32("eType").unwrap_or(0) == ET_NPC || entity >= MAX_CLIENTS {
                    game.npc_client_info(state).ok()
                } else {
                    game.client_info(usize::from(entity), siege_classes)
                }
            })
            .or_else(|| game.client_info(usize::from(entity), siege_classes))
            .ok_or_else(|| format!("custom sound {custom_name}: no client/NPC info for entity {entity}"))?;

        let profile = self.custom_sound_profile(&info)?;
        let without_marker = custom_name.strip_prefix('*').unwrap_or(custom_name);
        let stem = without_marker
            .strip_suffix(".wav")
            .or_else(|| without_marker.strip_suffix(".mp3"))
            .unwrap_or(without_marker);

        // TaystJK CG_CustomSound treats the jaPRO VGS table specially: VGS
        // tokens always resolve from the fixed generic male/female banks rather
        // than the speaking player's authored model sound directory.
        if crate::vgs::is_vgs_sound(custom_name) {
            let generic = if profile.female {
                "mp_generic_female"
            } else {
                "mp_generic_male"
            };
            let qpath = format!("sound/chars/{generic}/misc/{stem}");
            let sound = self.assets.register(&qpath)?;
            return Ok((qpath, sound));
        }

        let mut candidates = Vec::with_capacity(3);
        if !profile.sound_dir.is_empty() {
            candidates.push(format!("sound/chars/{}/misc/{stem}", profile.sound_dir));
        } else {
            candidates.push(format!("sound/chars/{}/misc/{stem}", info.model_name));
        }
        // Match CG_LoadCISounds' gender fallback if the authored model sound is
        // absent. SoundAssets.register tries both WAV and MP3 for extensionless paths.
        let generic = if profile.female { "mp_generic_female" } else { "mp_generic_male" };
        candidates.push(format!("sound/chars/{generic}/misc/{stem}"));

        let mut last_error = String::new();
        for qpath in candidates {
            match self.assets.register(&qpath) {
                Ok(sound) => return Ok((qpath, sound)),
                Err(error) => last_error = error,
            }
        }
        Err(format!("custom sound {custom_name} for {}/{} unavailable: {last_error}", info.model_name, info.skin_name))
    }

    fn custom_sound_profile(&mut self, info: &crate::cgame::ClientInfo) -> Result<CustomSoundProfile, String> {
        let key = format!("{}|{}", info.model_name, info.skin_name).to_ascii_lowercase();
        if let Some(profile) = self.custom_sound_profiles.get(&key) {
            return Ok(profile.clone());
        }

        let model = info.model_name.as_str();
        let skin = info.skin_name.as_str();
        let config_paths = if skin.is_empty() || skin.eq_ignore_ascii_case("default") {
            vec![
                format!("models/players/{model}/sounds.cfg"),
                format!("models/players/{model}/sounds_default.cfg"),
            ]
        } else {
            vec![
                format!("models/players/{model}/sounds_{skin}.cfg"),
                format!("models/players/{model}/sounds.cfg"),
            ]
        };

        let mut profile = CustomSoundProfile {
            sound_dir: model.to_owned(),
            female: info.female,
        };
        for config in config_paths {
            if let Some(text) = self.assets.read_text(&config, MAX_SOUND_CONFIG_BYTES)? {
                let mut lines = text.lines();
                if let Some(first) = lines.next() {
                    let first = first.trim();
                    if !first.is_empty() {
                        profile.sound_dir = first.to_owned();
                    }
                }
                // OpenJK scans the last line for an 'f' gender marker.
                if text.lines().last().is_some_and(|line| line.contains('f') || line.contains('F')) {
                    profile.female = true;
                }
                break;
            }
        }

        self.custom_sound_profiles.insert(key, profile.clone());
        Ok(profile)
    }

}

fn sound_request(event: &PresentationEvent, game: &ClientGameState) -> Result<Option<SoundRequest>, &'static str> {
    let mut entity = event.entity_num;
    let variant4 = event_variant(event, 4) + 1;
    let (qpath, channel, origin) = match event.event {
        2 => { // EV_FOOTSTEP: eventParm is the Raven material type.
            let stem = match event.parm {
                17 => "mud_walk",                          // MATERIAL_MUD
                7 => "dirt_step",                          // MATERIAL_DIRT
                8 => "sand_walk",                          // MATERIAL_SAND
                14 => "snow_step",                         // MATERIAL_SNOW
                5 | 6 => "grass_step",                     // short/long grass
                3 => "metal_step",                         // MATERIAL_SOLIDMETAL
                4 => "pipe_step",                          // MATERIAL_HOLLOWMETAL
                9 => "gravel_walk",                        // MATERIAL_GRAVEL
                21 | 22 | 24 | 25 | 27 => "rug_step",      // fabric/canvas/rubber/plastic/carpet
                1 | 2 => "wood_walk",                      // solid/hollow wood
                _ => "stone_step",
            };
            (format!("sound/player/footsteps/{stem}{variant4}.wav"), 6, SoundOrigin::Entity(event.position))
        }
        3 => (format!("sound/player/footsteps/metal_step{variant4}.wav"), 6, SoundOrigin::Entity(event.position)), // EV_FOOTSTEP_METAL
        // OpenJK maps splash, wade and swim events to FOOTSTEP_SPLASH here.
        4 | 5 | 6 => (format!("sound/player/footsteps/water_run{variant4}.wav"), 6, SoundOrigin::Entity(event.position)),
        11 => { // EV_FALL / DoFall. Hard falls also get *land1 via companion above.
            let path = if event.parm > 44 { "sound/player/fallsplat.wav" } else { "sound/player/land1.wav" };
            (path.to_owned(), 0, SoundOrigin::Entity(event.position))
        }
        16 => ("*jump1.wav".to_owned(), 3, SoundOrigin::Entity(event.position)), // EV_JUMP / CHAN_VOICE
        17 => ("sound/player/roll1.wav".to_owned(), 6, SoundOrigin::Entity(event.position)), // EV_ROLL / CHAN_BODY
        18 => ("sound/player/watr_in.wav".to_owned(), 0, SoundOrigin::Entity(event.position)),
        19 => ("sound/player/watr_out.wav".to_owned(), 0, SoundOrigin::Entity(event.position)),
        20 => ("sound/player/watr_un.wav".to_owned(), 0, SoundOrigin::Entity(event.position)),
        21 => ("*gasp.wav".to_owned(), 0, SoundOrigin::Entity(event.position)),
        75 => { // EV_VOICECMD_SOUND
            entity = u16::try_from(event.state.field_i32("groundEntityNum").unwrap_or(-1))
                .ok()
                .filter(|&number| number < MAX_CLIENTS)
                .ok_or("VOICE_SOUND_INVALID_CLIENT")?;
            let path = game.sound_qpath(event.parm).ok_or("SOUND_RESOURCE_MISSING")?;
            (path, 3, SoundOrigin::Entity(event.position)) // CHAN_VOICE
        }
        76 | 77 | 79 => {
            let channel = match event.event {
                76 => event.state.field_i32("saberEntityNum").unwrap_or(0),
                77 => 11, // CHAN_MENU1, positioned at the listener's head
                _ => {
                    entity = u16::try_from(event.state.field_i32("clientNum").unwrap_or(-1))
                        .ok().filter(|&n| n < 1024).ok_or("SOUND_INVALID_ENTITY")?;
                    event.state.field_i32("trickedentindex").unwrap_or(0)
                }
            };
            if channel >= 50 { return Err("TRACKED_SOUND_LOOP_PENDING"); }
            if channel < 0 { return Err("SOUND_INVALID_CHANNEL"); }
            let path = game.sound_qpath(event.parm).ok_or("SOUND_RESOURCE_MISSING")?;
            let origin = if event.event == 77 {
                entity = game.current_snapshot().and_then(|s| s.player_state.field_i32("clientNum"))
                    .and_then(|n| u16::try_from(n).ok()).unwrap_or(entity);
                SoundOrigin::Local
            } else { SoundOrigin::Entity(event.position) };
            (path, channel, origin)
        }
        40 => {
            let path = match event.parm {
                1 => "protecthit", 2 => "protect", 3 => "absorbhit", 4 => "absorb",
                5 => "jump", 6 => "grip", _ => return Err("PREDEFINED_SOUND_UNKNOWN"),
            };
            // EV_PREDEFSOUND explicitly supplies es->origin, not pos.trBase.
            let origin = super::entity_vec3(&event.state, "origin").unwrap_or(event.position);
            (format!("sound/weapons/force/{path}.mp3"), 0, SoundOrigin::Fixed(origin))
        }
        78 => return Err("GLOBAL_TEAM_SOUND_PENDING"), // enum, never a CS_SOUNDS index
        _ => return Ok(None),
    };
    Ok(Some(SoundRequest { qpath, entity, channel, origin }))
}

/// Presentational replacement for OpenJK's rand()/Q_irand selection. The exact
/// PRNG stream is not gameplay state; hashing immutable event data preserves the
/// same variant ranges without making render-frame order affect the choice.
fn event_variant(event: &PresentationEvent, count: usize) -> usize {
    debug_assert!(count > 0);
    let mixed = (event.server_time as u32)
        .wrapping_mul(0x9E37_79B9)
        ^ (u32::from(event.entity_num).wrapping_mul(0x85EB_CA6B))
        ^ (event.raw_event as u32).rotate_left(13)
        ^ (event.parm as u32).rotate_left(23);
    mixed as usize % count
}


fn audio_entities<'a>(
    entities: &'a [PresentedEntity],
    followed_entity: Option<&'a PresentedEntity>,
) -> impl Iterator<Item = &'a PresentedEntity> {
    let followed_number = followed_entity.map(|entity| entity.number);
    entities
        .iter()
        .filter(move |entity| Some(entity.number) != followed_number)
        .chain(followed_entity.into_iter())
}

/// OpenJK CG_EntityEffects "add loop sound". Brush models sit at their
/// inline-model midpoint offset by the mover's lerpOrigin; ET_SPEAKER takes
/// the ordinary S_AddLoopingSound path in MP (S_AddRealLoopingSound is
/// commented out there).
fn entity_loop(entity: &PresentedEntity, game: &ClientGameState, midpoints: &[[f32; 3]]) -> Option<LoopRequest> {
    let state = &entity.state;
    if state.field_i32("loopIsSoundset").unwrap_or(0) != 0 && entity.number >= MAX_CLIENTS {
        // loopSound selects BMS_START/MID/END of CS_AMBIENT_SET + soundSetIndex
        // via AS_GetBModelSound; ambient sets are not parsed yet.
        return None;
    }
    let loop_sound = state.field_i32("loopSound").unwrap_or(0);
    if loop_sound == 0 { return None; }
    let qpath = game.sound_qpath(loop_sound)?;
    if qpath.starts_with('*') { return None; } // custom player sounds never loop
    let origin = if state.field_i32("solid").unwrap_or(0) == SOLID_BMODEL {
        let midpoint = usize::try_from(state.field_i32("modelindex").unwrap_or(0))
            .ok()
            .and_then(|index| midpoints.get(index))
            .copied()
            .unwrap_or([0.0; 3]);
        std::array::from_fn(|i| entity.origin[i] + midpoint[i])
    } else {
        entity.origin
    };
    Some(LoopRequest { qpath, origin: SoundOrigin::Fixed(origin) })
}

/// OpenJK CG_Player saber hum: saber[0] hums while held with a lit blade;
/// saber[1] hums when lit unless it would duplicate saber[0]'s loop. The
/// listener's own hum is placed at the view origin (cg.refdef.vieworg).
/// Blade extension is not animated yet, so "has length" follows the same
/// saberHolstered rules the blade renderer uses.
fn saber_hum_loops(
    entity: &PresentedEntity,
    game: &ClientGameState,
    siege_classes: &[SiegeClassVisual],
    definitions: &SaberDefinitions,
    listener_entity: u16,
    out: &mut Vec<LoopRequest>,
) {
    let state = &entity.state;
    let holstered = state.field_i32("saberHolstered").unwrap_or(0);
    if state.field_i32("weapon").unwrap_or(0) != WP_SABER
        || holstered >= 2
        || state.field_i32("eFlags").unwrap_or(0) & EF_DEAD != 0
    {
        return;
    }
    let Some(info) = game.client_info(usize::from(entity.number), siege_classes) else { return };
    let origin = if entity.number == listener_entity {
        SoundOrigin::Local
    } else {
        SoundOrigin::Fixed(entity.origin)
    };
    let sound_loop = |name: &str| {
        definitions
            .get(name)
            .map(|definition| definition.sound_loop.clone())
            .unwrap_or_else(|| jka_assets::saber::SaberDefinition::openjk_default(name).sound_loop)
    };

    let mut first = None;
    if state.field_i32("saberInFlight").unwrap_or(0) == 0 {
        let qpath = sound_loop(&info.saber_name);
        out.push(LoopRequest { qpath: qpath.clone(), origin });
        first = Some(qpath);
    }
    // A dual saber's second blade is off at saberHolstered 1.
    if !info.saber2_name.is_empty() && !saber_name_is_removed(&info.saber2_name) && holstered == 0 {
        let qpath = sound_loop(&info.saber2_name);
        if first.as_deref() != Some(qpath.as_str()) {
            out.push(LoopRequest { qpath, origin });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jka_protocol::gamestate::{EntityState, ENTITY_FIELDS};

    fn event(event: i32, parm: i32) -> PresentationEvent {
        let mut state = EntityState { number: 3, fields: [0; ENTITY_FIELDS.len()] };
        if let Some(index) = ENTITY_FIELDS.iter().position(|(name, _)| *name == "eType") {
            state.fields[index] = ET_PLAYER as u32;
        }
        PresentationEvent {
            source_entity_num: 3,
            entity_num: 3,
            event,
            raw_event: event,
            parm,
            position: [1.0, 2.0, 3.0],
            event_only_entity: false,
            server_time: 1_000,
            state,
        }
    }

    #[test]
    fn followed_player_is_present_in_audio_entity_stream_once() {
        let make = |number| PresentedEntity {
            number,
            entity_type: ET_PLAYER,
            origin: [number as f32, 0.0, 0.0],
            angles: [0.0; 3],
            state: EntityState { number, fields: [0; ENTITY_FIELDS.len()] },
        };
        let packet_entities = vec![make(7), make(0)];
        let followed = make(0);
        let numbers: Vec<_> = audio_entities(&packet_entities, Some(&followed))
            .map(|entity| entity.number)
            .collect();
        assert_eq!(numbers, vec![7, 0]);
    }

    #[test]
    fn movement_events_use_openjk_channels_and_assets() {
        let game = ClientGameState::new();

        let jump = sound_request(&event(16, 0), &game).unwrap().unwrap();
        assert_eq!(jump.qpath, "*jump1.wav");
        assert_eq!(jump.channel, 3); // CHAN_VOICE

        let hard_fall = sound_request(&event(11, 50), &game).unwrap().unwrap();
        assert_eq!(hard_fall.qpath, "sound/player/fallsplat.wav");
        assert_eq!(hard_fall.channel, 0); // CHAN_AUTO

        let roll = sound_request(&event(17, 0), &game).unwrap().unwrap();
        assert_eq!(roll.qpath, "sound/player/roll1.wav");
        assert_eq!(roll.channel, 6); // CHAN_BODY
    }

    #[test]
    fn footstep_material_selects_openjk_pool() {
        let game = ClientGameState::new();
        let metal = sound_request(&event(2, 3), &game).unwrap().unwrap();
        assert!(metal.qpath.starts_with("sound/player/footsteps/metal_step"));
        assert!(metal.qpath.ends_with(".wav"));
        assert_eq!(metal.channel, 6);

        let wood = sound_request(&event(2, 1), &game).unwrap().unwrap();
        assert!(wood.qpath.starts_with("sound/player/footsteps/wood_walk"));
        assert_eq!(wood.channel, 6);
    }

    #[test]
    fn event_variant_is_stable_for_same_event() {
        let event = event(29, 7);
        assert_eq!(event_variant(&event, 8), event_variant(&event, 8));
        assert!(event_variant(&event, 8) < 8);
    }
}

//! CG_EntityEvent sound semantics, separated from the device/mixer implementation.
use crate::audio::{
    AudioBackend, AudioInfo, Listener, LoopRequest, SoundAssets, SoundDecodeJob,
    SoundDecodeResult, SoundOrigin, SoundRequest,
};
use crate::steam_audio::SteamAudioBakeData;
use jka_protocol::entity_event::EntityEvent;
use jka_assets::{
    bsp::AcousticMesh,
    pk3::AssetSearchPath,
    saber::{load_saber_definitions, SaberDefinition, SaberDefinitions},
    siege::SiegeClassVisual,
};
use std::{collections::{HashMap, HashSet, VecDeque}, sync::Arc};
use super::{
    ambient_sets::{AmbientFrame, AmbientRuntime, AmbientSets, AMBIENT_SET_FILENAME, BMS_MID, CS_AMBIENT_SET, CS_GLOBAL_AMBIENT_SET},
    event_presenter::EventDispatchResult,
    player_presenter::saber_name_is_removed,
    sound_tables::{self as tables, *},
    ClientGameState, PresentationEvent, PresentedEntity, EF_DEAD, ET_MISSILE, ET_NPC, ET_PLAYER,
};

const MAX_CLIENTS: u16 = 32;
const SOLID_BMODEL: i32 = 0x00ff_ffff;
const WP_SABER: i32 = 3;
const EF_ALT_FIRING: i32 = 1 << 10;
const CLASS_VEHICLE: i32 = 53;
const GT_CTY: i32 = 9;
const JAPRO_CINFO2_WTTRIBES: i32 = 1 << 4;
/// jaPRO `RS_TIMER_START` (`eFlags2`, `cg_raceSounds`): the race start-trigger sound.
const RS_TIMER_START: i32 = 1 << 0;
const DUEL_MUSIC: &str = "music/mp/duel.mp3";
const CS_MUSIC: u16 = 2;
const ENTITYNUM_NONE: i32 = 1023;
const FRAG_SOUND: &str = "sound/frag/frag.wav";
const FRAG_MIDAIR_SOUND: &str = "sound/frag/middy.wav";
const HIT_SOUNDS: [&str; 4] = [
    "sound/effects/hitsound.wav",
    "sound/effects/hitsound2.wav",
    "sound/effects/hitsound3.wav",
    "sound/effects/hitsound4.wav",
];
const HIT_TEAM_SOUND: &str = "sound/effects/hitsoundteam.wav";
const GT_CTF: i32 = 8;
const GT_TEAM: i32 = 6;
const GT_SIEGE: i32 = 7;
const GT_DUEL: i32 = 3;
const GT_POWERDUEL: i32 = 4;
const MAX_SOUNDBUFFER: usize = 20;
/// Gap `CG_PlayBufferedSounds` leaves between queued announcer lines.
const SOUNDBUFFER_SPACING_MS: i32 = 750;
/// `cgAnnouncerTime` hold-off after a timelimit/fraglimit line.
const ANNOUNCER_HOLD_MS: i32 = 3000;
const ONE_MINUTE_SOUND: &str = "sound/chars/protocol/misc/40MOM004";
const FIVE_MINUTE_SOUND: &str = "sound/chars/protocol/misc/40MOM005";
const ONE_FRAG_SOUND: &str = "sound/chars/protocol/misc/40MOM001";
const TWO_FRAG_SOUND: &str = "sound/chars/protocol/misc/40MOM002";
const THREE_FRAG_SOUND: &str = "sound/chars/protocol/misc/40MOM003";
const TIED_LEAD_SOUND: &str = "sound/chars/protocol/misc/40MOM032";
const IT_TEAM: i32 = 8;
const PS_WEAPON_READY: i32 = 0;
const MAX_SOUND_CONFIG_BYTES: usize = 4096;
const MAX_ENTITY_LOOPS: usize = 16; // MAX_CG_LOOPSOUNDS
const MAX_AMBIENT_SET_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
struct CustomSoundProfile {
    sound_dir: String,
    female: bool,
}

/// Immutable data event workers read while preparing sounds. It is loaded on
/// the owner thread (VFS) and only borrowed by workers.
#[derive(Default)]
pub(crate) struct SoundPrepData {
    pub saber_definitions: SaberDefinitions,
    pub ambient_sets: AmbientSets,
    pub game_sounds: crate::config::GameOptions,
}

/// Ordered-dispatch conditions that need per-entity presenter state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SoundGate {
    None,
    /// CG_PainEvent: at most one pain sound per 500 ms per entity.
    Pain,
    /// EV_JUMP: silent while the entity is mid voice line or just after pain/fall.
    Jump,
    /// EV_ITEM_PICKUP: `cg_entities[item].weapon` double-pickup guard (item entity).
    ItemPickup(i32),
}

/// Persistent sound state changes (the `CG_S_*` calls that outlive one event).
#[derive(Clone, Debug)]
pub(crate) enum SoundOp {
    /// CG_S_AddRealLoopingSound: keep `request.qpath` looping on the entity.
    LoopStart { entity: u16, request: SoundRequest },
    /// CG_S_StopLoopingSound(entity, -1).
    LoopStop { entity: u16 },
    /// S_MuteSound(entity, channel).
    Mute { entity: u16, channel: i32 },
    /// `S_StartBackgroundTrack("music/mp/duel.mp3", ...)`.
    DuelMusic,
    /// `CG_StartMusic(qtrue)`: back to the level's `CS_MUSIC` track.
    MapMusic,
    /// `CGCam_SetMusicMult(0.3, 5000)`: duck the music (dramatic failure / Jedi Master).
    MusicDip,
}

/// Everything one CG_EntityEvent asks of the sound system, decided purely from
/// event/game data so it can run on the event worker pool.
#[derive(Clone, Debug)]
pub(crate) struct PreparedSound {
    /// One-shot sounds in start order; `plays[0]` is the primary.
    pub plays: Vec<SoundRequest>,
    /// Replays the primary's resolved sound from another source (VGS radio copy).
    pub mirror: Option<SoundRequest>,
    /// Custom sound tried when the primary cannot be resolved (EV_TAUNT).
    pub fallback: Option<String>,
    pub gate: SoundGate,
    /// DoFall's `cent->pe.painTime = cg.time`.
    pub sets_pain_time: bool,
    pub ops: Vec<SoundOp>,
    /// `CG_AddBufferedSound`: queue the primary behind other announcer lines
    /// instead of starting it now.
    pub buffered: bool,
    /// (qpath, replacement) used when a play cannot be resolved (`*roll` -> `*jump1`).
    pub alt_qpaths: Vec<(String, String)>,
    pub result: EventDispatchResult,
}

impl PreparedSound {
    fn new(plays: Vec<SoundRequest>) -> Self {
        Self {
            plays,
            mirror: None,
            fallback: None,
            gate: SoundGate::None,
            sets_pain_time: false,
            ops: Vec::new(),
            buffered: false,
            alt_qpaths: Vec::new(),
            result: EventDispatchResult::Handled("SOUND_PLAYED"),
        }
    }

    fn with_ops(ops: Vec<SoundOp>) -> Self {
        let mut sound = Self::new(Vec::new());
        sound.ops = ops;
        sound
    }

    /// Every asset the owner thread should stage for worker decoding.
    pub(crate) fn requests(&self) -> impl Iterator<Item = &SoundRequest> {
        self.plays.iter().chain(self.ops.iter().filter_map(|op| match op {
            SoundOp::LoopStart { request, .. } => Some(request),
            _ => None,
        }))
    }
}

impl From<SoundRequest> for PreparedSound {
    fn from(request: SoundRequest) -> Self {
        Self::new(vec![request])
    }
}

pub(crate) type PreparedSoundEvent = Result<Option<PreparedSound>, &'static str>;

pub(crate) fn prepare_sound_event(
    event: &PresentationEvent,
    game: &ClientGameState,
    siege_classes: &[SiegeClassVisual],
    prep: &SoundPrepData,
) -> PreparedSoundEvent {
    let definitions = &prep.saber_definitions;
    match event.event {
        EntityEvent::EV_SABER_ATTACK => {
            saber_attack_request(definitions, event, game, siege_classes).map(|r| Some(r.into()))
        }
        EntityEvent::EV_SABER_HIT
        | EntityEvent::EV_SABER_BLOCK
        | EntityEvent::EV_SABER_CLASHFLARE => {
            saber_impact_request(definitions, event, game, siege_classes, prep.game_sounds.hit).map(|r| r.map(Into::into))
        }
        EntityEvent::EV_SABER_UNHOLSTER => Ok(saber_unholster_sounds(definitions, event, game, siege_classes)),
        _ => match complex_sound_event(event, game, siege_classes, prep) {
            Some(prepared) => prepared,
            None => sound_request(event, game).map(|r| r.map(Into::into)),
        },
    }
}

pub struct SoundPresenter {
    assets: SoundAssets,
    backend: Option<AudioBackend>,
    warnings: HashSet<String>,
    custom_sound_profiles: HashMap<String, CustomSoundProfile>,
    /// Last audible EV_PAIN per entity (CG_PainEvent's 500ms throttle).
    pain_times: HashMap<u16, i32>,
    prep: SoundPrepData,
    /// `cg_entities[item].weapon` pickup debounce (item entity -> expiry time).
    item_pickup_until: HashMap<i32, i32>,
    /// `cent->loopingSound[]` from CG_S_AddRealLoopingSound: persistent
    /// per-entity loops (resolved qpath) until EV_STOPLOOPINGSOUND / EV_MUTE_SOUND.
    event_loops: HashMap<u16, Vec<String>>,
    inline_model_midpoints: Arc<[[f32; 3]]>,
    announcer: Announcer,
    /// The `CS_MUSIC` value the level track was last started from.
    map_music: Option<String>,
    ambient: AmbientRuntime,
    epoch: std::time::Instant,
    /// `s_musicMult`: 1 normally, dipped by `MusicDip` and recovering 0.1 per 200 ms.
    music_mult: f32,
    music_mult_time: i32,
    music_dip_until: i32,
    /// `s_musicvolume` (after the unfocused mute).
    music_base: f32,
}

/// `cg.soundBuffer` plus the `cgAnnouncerTime` / limit-warning flags of
/// `CG_CheckLocalSounds`.
#[derive(Default)]
struct Announcer {
    queue: VecDeque<(SoundRequest, Arc<crate::audio::RegisteredSound>)>,
    /// `cg.soundTime`: earliest start of the next queued line.
    sound_time: i32,
    /// `cgAnnouncerTime`: no limit warning before this.
    announcer_time: i32,
    timelimit_warnings: u8,
    fraglimit_warnings: u8,
    /// Warnings restart with each level (CG_MapRestart).
    level_start_time: i32,
    /// (PERS_HITS, PERS_TEAM, viewed client) at the previous snapshot.
    last_hits: Option<(i32, i32, i32)>,
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
        let mut assets = SoundAssets::new(assets);
        let ambient_sets = load_ambient_sets(&mut assets);
        Self {
            assets,
            backend,
            warnings: HashSet::new(),
            custom_sound_profiles: HashMap::new(),
            pain_times: HashMap::new(),
            prep: SoundPrepData { saber_definitions, ambient_sets, game_sounds: Default::default() },
            item_pickup_until: HashMap::new(),
            event_loops: HashMap::new(),
            inline_model_midpoints: Arc::from(Vec::new()),
            announcer: Announcer::default(),
            map_music: None,
            ambient: AmbientRuntime::default(),
            epoch: std::time::Instant::now(),
            music_mult: 1.0,
            music_mult_time: 0,
            music_dip_until: 0,
            music_base: 0.25,
        }
    }

    /// OpenJK `cgs.inlineModelMidpoints` for the loaded BSP.
    pub fn set_inline_model_midpoints(&mut self, midpoints: Arc<[[f32; 3]]>) {
        self.inline_model_midpoints = midpoints;
    }

    /// `S_StartBackgroundTrack`: an optional intro followed by a looping track,
    /// streamed from the asset path. An empty intro stops the music.
    pub fn start_background_track(&mut self, intro: &str, looped: &str) {
        let Some(backend) = &mut self.backend else { return };
        let intro = intro.trim();
        if intro.is_empty() {
            backend.stop_music();
            return;
        }
        let looped = if looped.trim().is_empty() { intro } else { looped.trim() };
        let load = |assets: &mut SoundAssets, qpath: &str| assets.read_music(qpath);
        let looped_bytes = match load(&mut self.assets, looped) {
            Ok(bytes) => bytes,
            Err(error) => return self.warn_once(looped, &error),
        };
        // An intro identical to the loop is just the loop.
        let intro_bytes = if intro.eq_ignore_ascii_case(looped) {
            None
        } else {
            match load(&mut self.assets, intro) {
                Ok(bytes) => Some(bytes),
                Err(error) => {
                    self.warn_once(intro, &error);
                    None
                }
            }
        };
        if let Some(backend) = &mut self.backend {
            if let Err(error) = backend.start_music(intro_bytes, looped_bytes) {
                self.warn_once(intro, &error);
            }
        }
    }

    /// `CG_StartMusic`: start the level's `CS_MUSIC` track ("intro loop"). Unless
    /// `force`, an unchanged value keeps whatever plays (a duel track, say).
    fn start_map_music(&mut self, game: &ClientGameState, force: bool) {
        let value = game
            .configstring(CS_MUSIC)
            .map(|bytes| super::bytes_to_lossless_ascii(bytes))
            .unwrap_or_default();
        if !force && self.map_music.as_deref() == Some(value.as_str()) {
            return;
        }
        self.map_music = Some(value.clone());
        let mut words = value.split_whitespace().map(|word| word.trim_matches('"'));
        let intro = words.next().unwrap_or("");
        let looped = words.next().unwrap_or("");
        self.start_background_track(intro, looped);
    }

    pub fn set_music_volume(&mut self, level: f32) {
        self.music_base = level;
        self.apply_music_level();
    }

    fn apply_music_level(&mut self) {
        if let Some(backend) = &mut self.backend {
            backend.set_music_level(self.music_base * self.music_mult);
        }
    }

    /// CG_SE_UpdateMusic: once the dip time has passed the multiplier climbs back by 0.1 every 200 ms.
    fn update_music_mult(&mut self) {
        if self.music_mult >= 1.0 {
            return;
        }
        let now = self.epoch.elapsed().as_millis().min(i32::MAX as u128) as i32;
        if now < self.music_mult_time {
            return;
        }
        self.music_mult = (self.music_mult + 0.1).min(1.0);
        self.music_mult_time = now + 200;
        self.apply_music_level();
    }

    pub fn game_sounds(&self) -> crate::config::GameOptions {
        self.prep.game_sounds
    }

    pub fn set_game_sounds(&mut self, options: crate::config::GameOptions) {
        self.prep.game_sounds = options;
    }

    pub(crate) fn prep_data(&self) -> &SoundPrepData {
        &self.prep
    }

    pub fn retry_failed_assets(&mut self) -> Result<usize, String> {
        let retried = self.assets.retry_failed_assets()?;
        self.warnings.clear();
        self.custom_sound_profiles.clear();
        self.prep.ambient_sets = load_ambient_sets(&mut self.assets);
        Ok(retried)
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
        listener_origin: [f32; 3],
        server_time: i32,
    ) {
        if self.backend.is_none() { return; }
        let mut requests = Vec::new();
        let ambient = self.update_ambient(game, entities, followed_entity, listener_origin, server_time);
        requests.extend(ambient.loops.into_iter().map(|looped| LoopRequest {
            qpath: looped.qpath,
            origin: SoundOrigin::Fixed(looped.origin),
            volume: f32::from(looped.volume) / 255.0,
        }));
        for shot in ambient.shots {
            match self.assets.register(&shot.qpath) {
                Ok(sound) => {
                    if let Some(backend) = &mut self.backend {
                        backend.play_ambient(
                            SoundRequest {
                                qpath: shot.qpath,
                                entity: shot.entity,
                                channel: CHAN_AMBIENT,
                                origin: SoundOrigin::Fixed(shot.origin),
                            },
                            sound,
                            f32::from(shot.volume) / 255.0,
                        );
                    }
                }
                Err(error) => self.warn_once(&shot.qpath, &error),
            }
        }
        for entity in audio_entities(entities, followed_entity) {
            if let Some(request) = entity_loop(entity, game, &self.prep.ambient_sets, &self.inline_model_midpoints) {
                requests.push(request);
            }
            if entity.entity_type == ET_MISSILE {
                if let Some(request) = missile_loop(entity) {
                    requests.push(request);
                }
            }
            if let Some(loops) = self.event_loops.get(&entity.number) {
                let origin = SoundOrigin::Fixed(loop_origin(entity, &self.inline_model_midpoints));
                requests.extend(loops.iter().map(|qpath| LoopRequest { qpath: qpath.clone(), origin, volume: 1.0 }));
            }
            if entity.entity_type == ET_PLAYER {
                saber_hum_loops(entity, game, siege_classes, &self.prep.saber_definitions, listener_entity, &mut requests);
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

    /// `S_UpdateAmbientSet` for the worldspawn set plus `S_AddLocalSet` for every
    /// entity carrying a `soundSetIndex` (movers use theirs for door sounds only).
    fn update_ambient(
        &mut self,
        game: &ClientGameState,
        entities: &[PresentedEntity],
        followed_entity: Option<&PresentedEntity>,
        listener_origin: [f32; 3],
        server_time: i32,
    ) -> AmbientFrame {
        let mut frame = AmbientFrame::default();
        if !self.prep.game_sounds.ambient {
            return frame;
        }
        let now = self.epoch.elapsed().as_millis().min(i32::MAX as u128) as i32;
        let global = game
            .configstring(CS_GLOBAL_AMBIENT_SET)
            .map(super::bytes_to_lossless_ascii)
            .filter(|name| !name.trim().is_empty());
        if let Some(name) = global {
            self.ambient.update_global(&self.prep.ambient_sets, &name, listener_origin, now, &mut frame);
        }
        for entity in audio_entities(entities, followed_entity) {
            let index = entity.state.field_i32("soundSetIndex").unwrap_or(0);
            if index == 0 || entity.entity_type == super::ET_MOVER {
                continue;
            }
            let Some(name) = u16::try_from(index)
                .ok()
                .and_then(|index| CS_AMBIENT_SET.checked_add(index))
                .and_then(|cs| game.configstring(cs))
                .map(super::bytes_to_lossless_ascii)
                .filter(|name| !name.trim().is_empty())
            else {
                continue;
            };
            self.ambient.update_local(
                &self.prep.ambient_sets,
                &name,
                listener_origin,
                entity.origin,
                entity.number,
                server_time,
                &mut frame,
            );
        }
        frame
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

    /// `S_StartSound(NULL, clientNum, CHAN_BODY, sfx)`: one footstep of player
    /// `entity`, from `position` until the mixer knows where that player is.
    pub fn play_footstep(&mut self, qpath: &str, entity: u16, position: [f32; 3]) {
        const CHAN_BODY: i32 = 6;
        if self.backend.is_none() {
            return;
        }
        match self.assets.register(qpath) {
            Ok(sound) => {
                if let Some(backend) = &mut self.backend {
                    backend.play(
                        SoundRequest { qpath: qpath.to_owned(), entity, channel: CHAN_BODY, origin: SoundOrigin::Entity(position) },
                        sound,
                    );
                }
            }
            Err(error) => self.warn_once(qpath, &error),
        }
    }

    /// `S_StartLocalSound(sfx, CHAN_LOCAL_SOUND)`: a head-locked interface sound
    /// (chat beeps). Plays the first candidate that resolves, mirroring jaPRO's
    /// `privateChatSound` fallback to a base-assets sound.
    pub fn play_local_sound(&mut self, candidates: &[&str]) {
        const CHAN_LOCAL_SOUND: i32 = 8;
        if self.backend.is_none() {
            return;
        }
        for qpath in candidates {
            match self.assets.register(qpath) {
                Ok(sound) => {
                    if let Some(backend) = &mut self.backend {
                        backend.play(
                            SoundRequest { qpath: (*qpath).to_owned(), entity: 1023, channel: CHAN_LOCAL_SOUND, origin: SoundOrigin::Local },
                            sound,
                        );
                    }
                    return;
                }
                Err(error) => self.warn_once(qpath, &error),
            }
        }
    }

    /// `S_StartLocalSound(sfx, CHAN_ANNOUNCER)`: head-locked reward/announcer
    /// audio. The first registered candidate wins, matching the existing local
    /// UI-sound fallback behavior without bypassing SoundPresenter.
    pub fn play_announcer_sound(&mut self, candidates: &[&str]) {
        if self.backend.is_none() {
            return;
        }
        for qpath in candidates {
            match self.assets.register(qpath) {
                Ok(sound) => {
                    if let Some(backend) = &mut self.backend {
                        backend.play(
                            SoundRequest { qpath: (*qpath).to_owned(), entity: 1023, channel: CHAN_ANNOUNCER, origin: SoundOrigin::Local },
                            sound,
                        );
                    }
                    return;
                }
                Err(error) => self.warn_once(qpath, &error),
            }
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
    /// `CG_PlayBufferedSounds` and the timelimit/fraglimit announcer warnings of
    /// `CG_CheckLocalSounds`, run once per presentation frame at snapshot time.
    pub fn update_announcer(&mut self, game: &ClientGameState, siege_classes: &[SiegeClassVisual], now: i32) {
        if self.backend.is_none() {
            return;
        }
        self.update_hit_feedback(game, siege_classes);
        self.update_music_mult();
        self.start_map_music(game, false);
        let limits = game.match_limits();
        if limits.level_start_time != self.announcer.level_start_time {
            self.announcer.level_start_time = limits.level_start_time;
            self.announcer.timelimit_warnings = 0;
            self.announcer.fraglimit_warnings = 0;
        }
        let gametype = game.gametype();
        let mut warning = None; // (qpath, buffered)
        if self.announcer.announcer_time < now && limits.timelimit > 0 {
            let msec = now - limits.level_start_time;
            let warnings = self.announcer.timelimit_warnings;
            if warnings & 4 == 0 && msec > (limits.timelimit * 60 + 2) * 1000 {
                self.announcer.timelimit_warnings |= 1 | 2 | 4;
                warning = Some((TIED_LEAD_SOUND, false));
            } else if warnings & 2 == 0 && msec > (limits.timelimit - 1) * 60 * 1000 {
                self.announcer.timelimit_warnings |= 1 | 2;
                warning = Some((ONE_MINUTE_SOUND, false));
                self.announcer.announcer_time = now + ANNOUNCER_HOLD_MS;
            } else if limits.timelimit > 5 && warnings & 1 == 0 && msec > (limits.timelimit - 5) * 60 * 1000 {
                self.announcer.timelimit_warnings |= 1;
                warning = Some((FIVE_MINUTE_SOUND, false));
                self.announcer.announcer_time = now + ANNOUNCER_HOLD_MS;
            }
        }
        if warning.is_none()
            && limits.fraglimit > 0
            && gametype < GT_CTF
            && !matches!(gametype, GT_DUEL | GT_POWERDUEL | GT_SIEGE)
            && self.announcer.announcer_time < now
        {
            let mut high_score = limits.scores1;
            if gametype == GT_TEAM && limits.scores2 > high_score {
                high_score = limits.scores2;
            }
            let warnings = self.announcer.fraglimit_warnings;
            if warnings & 4 == 0 && high_score == limits.fraglimit - 1 {
                self.announcer.fraglimit_warnings |= 1 | 2 | 4;
                warning = Some((ONE_FRAG_SOUND, true));
            } else if limits.fraglimit > 2 && warnings & 2 == 0 && high_score == limits.fraglimit - 2 {
                self.announcer.fraglimit_warnings |= 1 | 2;
                warning = Some((TWO_FRAG_SOUND, true));
            } else if limits.fraglimit > 3 && warnings & 1 == 0 && high_score == limits.fraglimit - 3 {
                self.announcer.fraglimit_warnings |= 1;
                warning = Some((THREE_FRAG_SOUND, true));
            }
            if warning.is_some() {
                self.announcer.announcer_time = now + ANNOUNCER_HOLD_MS;
            }
        }
        if let (Some((qpath, buffered)), Some(entity)) = (warning, game.current_snapshot()
            .and_then(|snapshot| snapshot.player_state.field_i32("clientNum"))
            .and_then(|value| u16::try_from(value).ok()))
        {
            let mut request = SoundRequest { qpath: qpath.to_owned(), entity, channel: CHAN_ANNOUNCER, origin: SoundOrigin::Local };
            match self.register_request(game, siege_classes, &mut request) {
                Ok(found) if buffered => {
                    if self.announcer.queue.len() + 1 >= MAX_SOUNDBUFFER {
                        self.announcer.queue.pop_front();
                    }
                    self.announcer.queue.push_back((request, found));
                }
                Ok(found) => {
                    if let Some(backend) = &mut self.backend {
                        backend.play(request, found);
                    }
                }
                Err(error) => self.warn_once(&request.qpath, &error),
            }
        }
        // CG_PlayBufferedSounds.
        if self.announcer.sound_time < now {
            if let Some((request, found)) = self.announcer.queue.pop_front() {
                if let Some(backend) = &mut self.backend {
                    backend.play(request, found);
                }
                self.announcer.sound_time = now + SOUNDBUFFER_SPACING_MS;
            }
        }
    }

    /// `CG_CheckLocalSounds` hit changes (cg_hitsounds): a rise in PERS_HITS is a
    /// hit on an enemy, a fall is a hit on a teammate.
    fn update_hit_feedback(&mut self, game: &ClientGameState, siege_classes: &[SiegeClassVisual]) {
        const PERS_HITS: usize = 1;
        const PERS_TEAM: usize = 3;
        let Some(ps) = game.current_snapshot().map(|snapshot| &snapshot.player_state) else { return };
        let (hits, team) = (ps.persistant[PERS_HITS], ps.persistant[PERS_TEAM]);
        let previous = self.announcer.last_hits.replace((hits, team, ps.field_i32("clientNum").unwrap_or(-1)));
        let Some((old_hits, old_team, old_client)) = previous else { return };
        // A team change or a different viewed client resets the comparison.
        if old_team != team || old_client != ps.field_i32("clientNum").unwrap_or(-1) || hits == old_hits {
            return;
        }
        let level = self.prep.game_sounds.hit;
        let qpath = if hits > old_hits {
            match level {
                1..=4 => HIT_SOUNDS[usize::from(level) - 1],
                _ => return,
            }
        } else if level > 0 {
            HIT_TEAM_SOUND
        } else {
            return;
        };
        let Some(entity) = u16::try_from(ps.field_i32("clientNum").unwrap_or(-1)).ok() else { return };
        let mut request = SoundRequest { qpath: qpath.to_owned(), entity, channel: CHAN_LOCAL, origin: SoundOrigin::Local };
        match self.register_request(game, siege_classes, &mut request) {
            Ok(found) => {
                if let Some(backend) = &mut self.backend {
                    backend.play(request, found);
                }
            }
            Err(error) => self.warn_once(&request.qpath, &error),
        }
    }

    pub fn set_rate(&mut self, rate: f32) { if let Some(b) = &mut self.backend { b.set_rate(rate); } }
    pub fn clear(&mut self) {
        self.event_loops.clear();
        self.pain_times.clear();
        self.item_pickup_until.clear();
        self.announcer = Announcer::default();
        self.map_music = None;
        self.music_mult = 1.0;
        self.ambient.reset();
        if let Some(b) = &mut self.backend { b.clear(); }
    }
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

    /// Stage SFX referenced by an already-pure event preparation batch. VFS
    /// reads stay here on the owner thread; the returned compressed bytes are
    /// decoded by event workers. Custom `*voice` sounds resolve through the
    /// (cached) model sound profile first, so pain/death/taunt lines decode
    /// off-thread like every other sound.
    pub(crate) fn stage_prepared_asset_decodes<'a, I>(
        &mut self,
        sounds: I,
        game: &ClientGameState,
        siege_classes: &[SiegeClassVisual],
    ) -> Vec<SoundDecodeJob>
    where
        I: IntoIterator<Item = &'a PreparedSoundEvent>,
    {
        let mut seen = HashSet::new();
        let mut jobs = Vec::new();
        for prepared in sounds {
            let Ok(Some(sound)) = prepared else { continue };
            for request in sound.requests() {
                let candidates = if request.qpath.starts_with('*') {
                    match self.custom_sound_candidates(game, siege_classes, request.entity, &request.qpath) {
                        Ok(candidates) => candidates,
                        Err(_) => continue, // dispatch reports the same failure
                    }
                } else {
                    vec![request.qpath.clone()]
                };
                let key = candidates.join("|").replace('\\', "/").to_ascii_lowercase();
                if !seen.insert(key) {
                    continue;
                }
                if let Some(job) = self.assets.stage_first_available(&candidates) {
                    jobs.push(job);
                }
            }
        }
        jobs
    }

    pub(crate) fn queue_predecoded_assets<I>(&mut self, results: I)
    where
        I: IntoIterator<Item = SoundDecodeResult>,
    {
        self.assets.queue_predecoded(results);
    }

    /// Resolve one request to a decoded sound; custom `*` names are rewritten
    /// to the file that was actually found.
    fn register_request(
        &mut self,
        game: &ClientGameState,
        siege_classes: &[SiegeClassVisual],
        request: &mut SoundRequest,
    ) -> Result<Arc<crate::audio::RegisteredSound>, String> {
        if request.qpath.starts_with('*') {
            let (resolved_qpath, sound) =
                self.register_custom_sound(game, siege_classes, request.entity, &request.qpath)?;
            request.qpath = resolved_qpath;
            Ok(sound)
        } else {
            self.assets.register(&request.qpath)
        }
    }

    /// Persistent state changes from an event (loops, mutes), applied in
    /// receive order even when the one-shot part of the event is stale.
    fn apply_sound_ops(&mut self, ops: &[SoundOp], game: &ClientGameState, siege_classes: &[SiegeClassVisual]) {
        for op in ops {
            match op {
                SoundOp::LoopStart { entity, request } => {
                    let mut request = request.clone();
                    match self.register_request(game, siege_classes, &mut request) {
                        Ok(_) => {
                            let loops = self.event_loops.entry(*entity).or_default();
                            if !loops.contains(&request.qpath) && loops.len() < MAX_ENTITY_LOOPS {
                                loops.push(request.qpath);
                            }
                        }
                        Err(error) => self.warn_once(&request.qpath, &error),
                    }
                }
                SoundOp::LoopStop { entity } => {
                    self.event_loops.remove(entity);
                }
                SoundOp::Mute { entity, channel } => {
                    if let Some(backend) = &mut self.backend {
                        backend.mute(*entity, *channel);
                    }
                }
                SoundOp::DuelMusic => self.start_background_track(DUEL_MUSIC, DUEL_MUSIC),
                SoundOp::MapMusic => self.start_map_music(game, true),
                SoundOp::MusicDip => {
                    self.music_dip_until = self.epoch.elapsed().as_millis().min(i32::MAX as u128) as i32 + 5000;
                    self.music_mult = 0.3;
                    self.music_mult_time = self.music_dip_until;
                    self.apply_music_level();
                }
            }
        }
    }

    pub(crate) fn dispatch_prepared(
        &mut self,
        event: &PresentationEvent,
        game: &ClientGameState,
        siege_classes: &[SiegeClassVisual],
        now: i32,
        prepared: PreparedSoundEvent,
    ) -> Option<EventDispatchResult> {
        let mut sound = match prepared {
            Ok(Some(sound)) => sound,
            Ok(None) => return None,
            Err(reason) => return Some(EventDispatchResult::Partial(reason)),
        };
        if self.backend.is_none() {
            return Some(EventDispatchResult::Partial("AUDIO_DEVICE_UNAVAILABLE"));
        }
        self.apply_sound_ops(&sound.ops, game, siege_classes);
        if sound.plays.is_empty() {
            return Some(sound.result);
        }
        // Avoid a burst of old sounds after a stalled frame or fast-forward.
        if now.saturating_sub(event.server_time) > 250 {
            return Some(EventDispatchResult::Partial("SOUND_SKIPPED_CATCHUP"));
        }

        let entity = sound.plays[0].entity;
        let recent_pain = |presenter: &Self| {
            presenter
                .pain_times
                .get(&entity)
                .is_some_and(|&last| (0..500).contains(&event.server_time.saturating_sub(last)))
        };
        match sound.gate {
            SoundGate::None => {}
            SoundGate::Pain => {
                // CG_PainEvent: no more than two pain sounds a second per entity.
                if recent_pain(self) {
                    return Some(EventDispatchResult::Partial("PAIN_SOUND_THROTTLED"));
                }
            }
            SoundGate::Jump => {
                // EV_JUMP: "don't play over other sounds" / "not right after pain or a fall".
                if recent_pain(self) || self.backend.as_ref().is_some_and(|b| b.voice_active(entity)) {
                    return Some(EventDispatchResult::Partial("JUMP_SOUND_SUPPRESSED"));
                }
            }
            SoundGate::ItemPickup(item) => {
                // rww's double-pickup hack: the item entity is deaf for 500 ms.
                if self.item_pickup_until.get(&item).is_some_and(|&until| until >= event.server_time) {
                    return Some(EventDispatchResult::Partial("ITEM_PICKUP_DEBOUNCED"));
                }
                self.item_pickup_until.insert(item, event.server_time + 500);
            }
        }
        if sound.gate == SoundGate::Pain || sound.sets_pain_time {
            self.pain_times.insert(entity, event.server_time);
        }

        let mut resolved: Vec<(SoundRequest, Arc<crate::audio::RegisteredSound>)> = Vec::with_capacity(sound.plays.len());
        let mut primary_sound = None;
        for (index, mut request) in std::mem::take(&mut sound.plays).into_iter().enumerate() {
            let mut result = self.register_request(game, siege_classes, &mut request);
            if result.is_err() {
                if let Some((_, alt)) = sound.alt_qpaths.iter().find(|(from, _)| *from == request.qpath) {
                    let mut alt_request = SoundRequest { qpath: alt.clone(), ..request.clone() };
                    if let Ok(found) = self.register_request(game, siege_classes, &mut alt_request) {
                        request = alt_request;
                        result = Ok(found);
                    }
                }
            }
            if result.is_err() && index == 0 {
                if let Some(fallback) = sound.fallback.take() {
                    // The chosen anger/taunt/gloat variant may not exist for this model.
                    let mut fallback_request = SoundRequest { qpath: fallback, ..request.clone() };
                    if let Ok(found) = self.register_request(game, siege_classes, &mut fallback_request) {
                        request = fallback_request;
                        result = Ok(found);
                    }
                }
            }
            match result {
                Ok(found) => {
                    if index == 0 {
                        primary_sound = Some(Arc::clone(&found));
                    }
                    resolved.push((request, found));
                }
                Err(error) => self.warn_once(&request.qpath, &error),
            }
        }
        if resolved.is_empty() {
            return Some(EventDispatchResult::Partial("SOUND_ASSET_UNAVAILABLE"));
        }

        if sound.buffered {
            if let Some(line) = resolved.into_iter().next() {
                if self.announcer.queue.len() + 1 >= MAX_SOUNDBUFFER {
                    self.announcer.queue.pop_front(); // the ring overwrites the oldest line
                }
                self.announcer.queue.push_back(line);
            }
            return Some(sound.result);
        }
        if let Some(backend) = &mut self.backend {
            if let (Some(mirror), Some(found)) = (sound.mirror.take(), primary_sound) {
                backend.play(mirror, found);
            }
            for (request, found) in resolved {
                backend.play(request, found);
            }
        }
        Some(sound.result)
    }

    /// OpenJK CG_CustomSound/CG_LoadCISounds lookup order for protocol sounds
    /// whose CS_SOUNDS entry begins with '*': the entity's model/skin selects
    /// models/players/<model>/sounds*.cfg, then the named voice resolves under
    /// sound/chars/<soundpath>/misc with the male/female generic bank as fallback.
    fn custom_sound_candidates(
        &mut self,
        game: &ClientGameState,
        siege_classes: &[SiegeClassVisual],
        entity: u16,
        custom_name: &str,
    ) -> Result<Vec<String>, String> {
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
            let generic = if profile.female { "mp_generic_female" } else { "mp_generic_male" };
            return Ok(vec![format!("sound/chars/{generic}/misc/{stem}")]);
        }

        let mut candidates = Vec::with_capacity(2);
        if !profile.sound_dir.is_empty() {
            candidates.push(format!("sound/chars/{}/misc/{stem}", profile.sound_dir));
        } else {
            candidates.push(format!("sound/chars/{}/misc/{stem}", info.model_name));
        }
        // Match CG_LoadCISounds' gender fallback if the authored model sound is
        // absent. SoundAssets.register tries both WAV and MP3 for extensionless paths.
        let generic = if profile.female { "mp_generic_female" } else { "mp_generic_male" };
        candidates.push(format!("sound/chars/{generic}/misc/{stem}"));
        Ok(candidates)
    }

    fn register_custom_sound(
        &mut self,
        game: &ClientGameState,
        siege_classes: &[SiegeClassVisual],
        entity: u16,
        custom_name: &str,
    ) -> Result<(String, Arc<crate::audio::RegisteredSound>), String> {
        let candidates = self.custom_sound_candidates(game, siege_classes, entity, custom_name)?;
        let mut last_error = String::new();
        for qpath in candidates {
            match self.assets.register(&qpath) {
                Ok(sound) => return Ok((qpath, sound)),
                Err(error) => last_error = error,
            }
        }
        let (model, skin) = game
            .client_info(usize::from(entity), siege_classes)
            .map(|info| (info.model_name, info.skin_name))
            .unwrap_or_default();
        Err(format!("custom sound {custom_name} for {model}/{skin} unavailable: {last_error}"))
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

fn load_ambient_sets(assets: &mut SoundAssets) -> AmbientSets {
    match assets.read_text(AMBIENT_SET_FILENAME, MAX_AMBIENT_SET_BYTES) {
        Ok(Some(text)) => AmbientSets::parse(&text),
        Ok(None) => AmbientSets::default(),
        Err(error) => {
            eprintln!("AUDIO AMBIENT SETS UNAVAILABLE (door/mover sound sets silent): {error}");
            AmbientSets::default()
        }
    }
}

fn impact_saber_definition(
    saber_definitions: &SaberDefinitions,
    event: &PresentationEvent,
    game: &ClientGameState,
    siege_classes: &[SiegeClassVisual],
) -> Option<(SaberDefinition, bool)> {
    let owner = event.state.field_i32("otherEntityNum2")?;
    if !(0..1023).contains(&owner) {
        return None;
    }
    let owner = u16::try_from(owner).ok()?;
    let info = game
        .entity_state(owner)
        .and_then(|state| {
            if state.field_i32("eType").unwrap_or(0) == ET_NPC {
                game.npc_client_info(state).ok()
            } else {
                game.client_info(usize::from(owner), siege_classes)
            }
        })
        .or_else(|| game.client_info(usize::from(owner), siege_classes))?;
    let saber_num = event.state.field_i32("weapon").unwrap_or(0);
    let saber_name = match saber_num {
        0 => &info.saber_name,
        1 => &info.saber2_name,
        _ => return None,
    };
    let definition = saber_definitions.get(saber_name)?.clone();
    let blade_num = event.state.field_i32("legsAnim").unwrap_or(0).max(0) as usize;
    let second_style = definition.blade_style2_start > 0
        && blade_num >= definition.blade_style2_start;
    Some((definition, second_style))
}

/// OpenJK EV_SABER_HIT / EV_SABER_BLOCK / EV_SABER_CLASHFLARE audio.
fn saber_impact_request(
    saber_definitions: &SaberDefinitions,
    event: &PresentationEvent,
    game: &ClientGameState,
    siege_classes: &[SiegeClassVisual],
    hit_sounds: u8,
) -> Result<Option<SoundRequest>, &'static str> {
    let origin = SoundOrigin::Fixed(
        super::entity_vec3(&event.state, "origin").unwrap_or(event.position),
    );
    match event.event {
        EntityEvent::EV_SABER_HIT => {
            // OpenJK EV_SABER_HIT only starts hitSound for flesh/special hits.
            if event.parm == 0 {
                return Ok(None);
            }
            // cg_hitsounds 5: always the plain saberhit; 6: any of the four; else 1..=3.
            let variant = match hit_sounds {
                5 => 0,
                6 => event_variant(event, 4),
                _ => event_variant(event, 3) + 1,
            };
            let mut qpath = if variant == 0 {
                "sound/weapons/saber/saberhit.wav".to_owned()
            } else {
                format!("sound/weapons/saber/saberhit{variant}.wav")
            };
            if let Some((definition, second_style)) =
                impact_saber_definition(saber_definitions, event, game, siege_classes)
            {
                let sounds = if second_style {
                    &definition.hit2_sounds
                } else {
                    &definition.hit_sounds
                };
                if sounds[0].is_some() {
                    if let Some(custom) = sounds[event_variant(event, 3)]
                        .clone()
                        .or_else(|| sounds[0].clone())
                    {
                        qpath = custom;
                    }
                }
            }
            Ok(Some(SoundRequest {
                qpath,
                entity: event.entity_num,
                channel: 0, // CHAN_AUTO
                origin,
            }))
        }
        EntityEvent::EV_SABER_BLOCK => {
            if event.parm == 0 {
                return Ok(None); // projectile deflect sound is authored by the EFX
            }
            let mut qpath = format!(
                "sound/weapons/saber/saberblock{}.wav",
                event_variant(event, 9) + 1
            );
            if let Some((definition, second_style)) =
                impact_saber_definition(saber_definitions, event, game, siege_classes)
            {
                let sounds = if second_style {
                    &definition.block2_sounds
                } else {
                    &definition.block_sounds
                };
                if sounds[0].is_some() {
                    if let Some(custom) = sounds[event_variant(event, 3)]
                        .clone()
                        .or_else(|| sounds[0].clone())
                    {
                        qpath = custom;
                    }
                }
            }
            Ok(Some(SoundRequest {
                qpath,
                entity: event.entity_num,
                channel: 0, // CHAN_AUTO
                origin,
            }))
        }
        EntityEvent::EV_SABER_CLASHFLARE => Ok(Some(SoundRequest {
            qpath: format!(
                "sound/weapons/saber/saberhitwall{}.wav",
                event_variant(event, 3) + 1
            ),
            entity: 1023, // ENTITYNUM_NONE
            channel: 2,  // CHAN_WEAPON
            origin,
        })),
        _ => Ok(None),
    }
}

fn saber_attack_request(
    saber_definitions: &SaberDefinitions,
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
        .and_then(|info| saber_definitions.get(&info.saber_name))
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
    Ok(SoundRequest { qpath, entity: event.entity_num, channel: 2, origin })
}

fn sound_request(event: &PresentationEvent, game: &ClientGameState) -> Result<Option<SoundRequest>, &'static str> {
    let mut entity = event.entity_num;
    let variant4 = event_variant(event, 4) + 1;
    let (qpath, channel, origin) = match event.event {
        EntityEvent::EV_FOOTSTEP => { // eventParm is the Raven material type.
            let material = u32::try_from(event.parm).unwrap_or(0);
            let path = super::footsteps::material_step(material).sound.path(false, variant4 - 1);
            (path, 6, SoundOrigin::Entity(event.position))
        }
        EntityEvent::EV_FOOTSTEP_METAL => (format!("sound/player/footsteps/metal_step{variant4}.wav"), 6, SoundOrigin::Entity(event.position)), // EV_FOOTSTEP_METAL
        // OpenJK maps splash, wade and swim events to FOOTSTEP_SPLASH here.
        EntityEvent::EV_FOOTSPLASH | EntityEvent::EV_FOOTWADE | EntityEvent::EV_SWIM => (format!("sound/player/footsteps/water_run{variant4}.wav"), 6, SoundOrigin::Entity(event.position)),
        EntityEvent::EV_FALL => { // DoFall. Hard falls also get *land1 via companion above.
            let path = if event.parm > 44 { "sound/player/fallsplat.wav" } else { "sound/player/land1.wav" };
            (path.to_owned(), 0, SoundOrigin::Entity(event.position))
        }
        EntityEvent::EV_JUMP => ("*jump1.wav".to_owned(), 3, SoundOrigin::Entity(event.position)), // EV_JUMP / CHAN_VOICE
        EntityEvent::EV_ROLL => ("sound/player/roll1.wav".to_owned(), 6, SoundOrigin::Entity(event.position)), // EV_ROLL / CHAN_BODY
        EntityEvent::EV_WATER_TOUCH => ("sound/player/watr_in.wav".to_owned(), 0, SoundOrigin::Entity(event.position)),
        EntityEvent::EV_WATER_LEAVE => ("sound/player/watr_out.wav".to_owned(), 0, SoundOrigin::Entity(event.position)),
        EntityEvent::EV_WATER_UNDER => ("sound/player/watr_un.wav".to_owned(), 0, SoundOrigin::Entity(event.position)),
        EntityEvent::EV_WATER_CLEAR => ("*gasp.wav".to_owned(), 0, SoundOrigin::Entity(event.position)),
        EntityEvent::EV_VOICECMD_SOUND => {
            entity = u16::try_from(event.state.field_i32("groundEntityNum").unwrap_or(-1))
                .ok()
                .filter(|&number| number < MAX_CLIENTS)
                .ok_or("VOICE_SOUND_INVALID_CLIENT")?;
            let path = game.sound_qpath(event.parm).ok_or("SOUND_RESOURCE_MISSING")?;
            (path, 3, SoundOrigin::Entity(event.position)) // CHAN_VOICE
        }
        EntityEvent::EV_GENERAL_SOUND | EntityEvent::EV_GLOBAL_SOUND | EntityEvent::EV_ENTITY_SOUND => {
            let channel = match event.event {
                EntityEvent::EV_GENERAL_SOUND => event.state.field_i32("saberEntityNum").unwrap_or(0),
                EntityEvent::EV_GLOBAL_SOUND => CHAN_MENU1, // positioned at the listener's head
                _ => {
                    entity = u16::try_from(event.state.field_i32("clientNum").unwrap_or(-1))
                        .ok().filter(|&n| n < 1024).ok_or("SOUND_INVALID_ENTITY")?;
                    event.state.field_i32("trickedentindex").unwrap_or(0)
                }
            };
            if channel < 0 { return Err("SOUND_INVALID_CHANNEL"); }
            let path = game.sound_qpath(event.parm).ok_or("SOUND_RESOURCE_MISSING")?;
            let origin = if event.event == EntityEvent::EV_GLOBAL_SOUND {
                entity = game.current_snapshot().and_then(|s| s.player_state.field_i32("clientNum"))
                    .and_then(|n| u16::try_from(n).ok()).unwrap_or(entity);
                SoundOrigin::Local
            } else { SoundOrigin::Entity(event.position) };
            (path, channel, origin)
        }
        EntityEvent::EV_PREDEFSOUND => {
            let path = tables::predefined_sound(event.parm).ok_or("PREDEFINED_SOUND_UNKNOWN")?;
            // EV_PREDEFSOUND explicitly supplies es->origin, not pos.trBase.
            let origin = super::entity_vec3(&event.state, "origin").unwrap_or(event.position);
            (path.to_owned(), CHAN_AUTO, SoundOrigin::Fixed(origin))
        }
        EntityEvent::EV_GLOBAL_TEAM_SOUND => return Err("GLOBAL_TEAM_SOUND_PENDING"), // enum, never a CS_SOUNDS index
        // CG_PainEvent: health (eventParm) selects the pain bucket. The
        // two-per-second throttle needs per-entity state and lives in dispatch.
        EntityEvent::EV_PAIN => {
            let name = match event.parm {
                ..=24 => "pain25",
                25..=49 => "pain50",
                50..=74 => "pain75",
                _ => "pain100",
            };
            (format!("*{name}.wav"), 3, SoundOrigin::Entity(event.position))
        }
        EntityEvent::EV_TAUNT => (taunt_custom_sound(event), 3, SoundOrigin::Entity(event.position)),
        other => match voice_event_custom_sound(other) {
            // CG_TryPlayCustomSound / EV_DEATHx: entity-relative CHAN_VOICE.
            Some(qpath) => (qpath, 3, SoundOrigin::Entity(event.position)),
            None => return Ok(None),
        },
    };
    Ok(Some(SoundRequest { qpath, entity, channel, origin }))
}

/// `*name.wav` for the fixed-index custom voice events (EV_DEATHx and the
/// NPC/player bark families), mirroring CG_EntityEvent's `event - EV_X1 + 1`.
pub(super) fn voice_event_custom_sound(event: EntityEvent) -> Option<String> {
    use EntityEvent as E;
    let families: &[(EntityEvent, &str)] = &[
        (E::EV_DEATH1, "death"), (E::EV_ANGER1, "anger"), (E::EV_VICTORY1, "victory"),
        (E::EV_CONFUSE1, "confuse"), (E::EV_PUSHED1, "pushed"), (E::EV_CHOKE1, "choke"),
        (E::EV_CHASE1, "chase"), (E::EV_COVER1, "cover"), (E::EV_DETECTED1, "detected"),
        (E::EV_GIVEUP1, "giveup"), (E::EV_LOOK1, "look"), (E::EV_OUTFLANK1, "outflank"),
        (E::EV_ESCAPING1, "escaping"), (E::EV_SIGHT1, "sight"), (E::EV_SOUND1, "sound"),
        (E::EV_SUSPICIOUS1, "suspicious"), (E::EV_COMBAT1, "combat"),
        (E::EV_JDETECTED1, "jdetected"), (E::EV_TAUNT1, "taunt"), (E::EV_JCHASE1, "jchase"),
        (E::EV_JLOST1, "jlost"), (E::EV_DEFLECT1, "deflect"), (E::EV_GLOAT1, "gloat"),
    ];
    // Family lengths are what separates adjacent enum runs.
    let lengths = [3, 3, 3, 3, 3, 3, 3, 5, 5, 4, 2, 2, 3, 3, 3, 5, 3, 3, 3, 3, 3, 3, 3];
    let value = event.as_i32();
    for ((first, name), len) in families.iter().zip(lengths) {
        let offset = value - first.as_i32();
        if (0..len).contains(&offset) {
            return Some(format!("*{name}{}.wav", offset + 1));
        }
    }
    match event {
        E::EV_FFWARN => Some("*ffwarn.wav".to_owned()),
        E::EV_FFTURN => Some("*ffturn.wav".to_owned()),
        E::EV_LOST1 => Some("*lost1.wav".to_owned()),
        E::EV_PUSHFAIL => Some("*pushfail.wav".to_owned()),
        _ => None,
    }
}

/// TaystJK EV_TAUNT sound choice (eventParm = taunt type). The original tries
/// alternatives with fallbacks; the primary pick is made here and dispatch
/// falls back to plain `*taunt.wav` if the model lacks it.
fn taunt_custom_sound(event: &PresentationEvent) -> String {
    let n = event_variant(event, 3) + 1;
    let alt = event_variant(event, 2) == 0;
    match event.parm {
        3 => { // TAUNT_FLOURISH
            let family = if alt { "deflect" } else { "gloat" };
            format!("*{family}{n}.wav")
        }
        4 => format!("*victory{n}.wav"), // TAUNT_GLOAT
        1 | 2 => "*taunt.wav".to_owned(), // TAUNT_BOW / TAUNT_MEDITATE have no sound of their own
        _ => match event_variant(event, 4) {
            0 => "*taunt.wav".to_owned(),
            num => format!("*{}{num}.wav", if alt { "anger" } else { "taunt" }),
        },
    }
}

/// cg_jumpSounds / cg_rollSounds: 1 everyone, 2 other players, 3 only yourself.
fn own_or_other_allowed(option: u8, own: bool) -> bool {
    match option {
        1 => true,
        2 => !own,
        3 => own,
        _ => false,
    }
}

pub(super) fn local_client(game: &ClientGameState) -> Option<u16> {
    game.current_snapshot()
        .and_then(|snapshot| snapshot.player_state.field_i32("clientNum"))
        .and_then(|value| u16::try_from(value).ok())
}

/// A field of the viewed player's playerState (`cg.snap->ps.<field>`).
pub(super) fn local_ps(game: &ClientGameState, field: &str) -> i32 {
    game.current_snapshot()
        .and_then(|snapshot| snapshot.player_state.field_i32(field))
        .unwrap_or(0)
}

/// `cg.predictedPlayerState.duelInProgress && clientNum != c && duelIndex != c`:
/// the event belongs to someone outside the viewer's duel.
pub(super) fn duel_hides(game: &ClientGameState, client: i32) -> bool {
    local_ps(game, "duelInProgress") != 0
        && local_client(game).map_or(-1, i32::from) != client
        && local_ps(game, "duelIndex") != client
}

fn local_sound(game: &ClientGameState, qpath: &str, channel: i32) -> Option<SoundRequest> {
    let entity = local_client(game)?;
    Some(SoundRequest { qpath: qpath.to_owned(), entity, channel, origin: SoundOrigin::Local })
}

/// BG_InKnockDownOnly(legsAnim): BOTH_KNOCKDOWN1..5.
fn in_knockdown_only(anim: i32) -> bool {
    jka_movement::animation_name(anim).is_some_and(|name| {
        name.strip_prefix("BOTH_KNOCKDOWN")
            .and_then(|suffix| suffix.parse::<i32>().ok())
            .is_some_and(|n| (1..=5).contains(&n))
    })
}

/// CG_EntityEvent DoFall: the landing sound(s) for `eventParm` fall delta, and
/// whether the entity's pain time is stamped (suppressing an immediate pain/jump voice).
fn fall_sounds(event: &PresentationEvent, ski: bool) -> (Vec<SoundRequest>, bool) {
    const ANIM_TOGGLEBIT: i32 = 2048;
    let delta = event.parm;
    let origin = SoundOrigin::Entity(event.position);
    let sound = |qpath: &str, channel: i32| SoundRequest {
        qpath: qpath.to_owned(),
        entity: event.entity_num,
        channel,
        origin,
    };
    let flags = event.state.field_i32("eFlags").unwrap_or(0);
    let legs = event.state.field_i32("legsAnim").unwrap_or(0) & !ANIM_TOGGLEBIT;
    if flags & EF_DEAD != 0 {
        // Corpses crack into the ground.
        (vec![sound(if delta > 25 { FALL_SOUND } else { OBJECT_HIT_SOUND }, CHAN_AUTO)], false)
    } else if in_knockdown_only(legs) {
        (vec![sound(if delta > 14 { FALL_SOUND } else { OBJECT_HIT_SOUND }, CHAN_AUTO)], false)
    } else if delta > 44 {
        (vec![sound(FALL_SOUND, CHAN_AUTO), sound("*land1.wav", CHAN_VOICE)], true)
    } else if delta == 1 && ski {
        // jaPRO tribes ski landing.
        (vec![sound("sound/player/ski_soft.wav", CHAN_AUTO)], false)
    } else {
        (vec![sound(LAND_SOUND, CHAN_AUTO)], false)
    }
}

/// `CG_InClientBitflags`: the 64-client mask packed into trickedentindex[1-4].
fn in_client_bitflags(state: &jka_protocol::gamestate::EntityState, client: i32) -> bool {
    let (field, sub) = match client {
        48.. => ("trickedentindex4", 48),
        32..=47 => ("trickedentindex3", 32),
        16..=31 => ("trickedentindex2", 16),
        _ => ("trickedentindex", 0),
    };
    state.field_i32(field).unwrap_or(0) & (1 << (client - sub)) != 0
}

/// CS_AMBIENT_SET name of an entity's `soundSetIndex`, resolved to its BMS stage sound.
fn bmodel_stage_sound(
    state: &jka_protocol::gamestate::EntityState,
    game: &ClientGameState,
    prep: &SoundPrepData,
    stage: i32,
) -> Option<String> {
    let index = u16::try_from(state.field_i32("soundSetIndex").unwrap_or(0)).ok().filter(|&i| i != 0)?;
    let set = game.configstring(CS_AMBIENT_SET.checked_add(index)?)?;
    let set = String::from_utf8_lossy(set);
    let set = set.trim();
    if set.is_empty() {
        return None;
    }
    prep.ambient_sets.bmodel_sound(set, stage).map(str::to_owned)
}

fn saber_unholster_sounds(
    definitions: &SaberDefinitions,
    event: &PresentationEvent,
    game: &ClientGameState,
    siege_classes: &[SiegeClassVisual],
) -> Option<PreparedSound> {
    let info = if event.state.field_i32("eType").unwrap_or(ET_PLAYER) == ET_NPC {
        game.npc_client_info(&event.state).ok()
    } else if event.entity_num < MAX_CLIENTS {
        game.client_info(usize::from(event.entity_num), siege_classes)
    } else {
        None
    }?;
    let request = |qpath: String| SoundRequest {
        qpath,
        entity: event.entity_num,
        channel: CHAN_AUTO,
        origin: SoundOrigin::Entity(event.position),
    };
    let mut plays = vec![request(definitions.definition_or_default(&info.saber_name).sound_on)];
    if !info.saber2_name.is_empty() && !saber_name_is_removed(&info.saber2_name) {
        plays.push(request(definitions.definition_or_default(&info.saber2_name).sound_on));
    }
    Some(PreparedSound::new(plays))
}

/// Events whose sound needs more than one `SoundRequest`, gating on presenter
/// state, persistent loops, or viewer state. `None` means the event is a plain
/// one-shot handled by `sound_request`.
fn complex_sound_event(
    event: &PresentationEvent,
    game: &ClientGameState,
    siege_classes: &[SiegeClassVisual],
    prep: &SoundPrepData,
) -> Option<PreparedSoundEvent> {
    use EntityEvent as E;
    let es = &event.state;
    let entity = event.entity_num;
    let at_entity = SoundOrigin::Entity(event.position);
    let client_num = es.field_i32("clientNum").unwrap_or(-1);
    let number = i32::from(event.entity_num);
    let none: Option<PreparedSoundEvent> = Some(Ok(None));
    let one = |request: SoundRequest| -> Option<PreparedSoundEvent> { Some(Ok(Some(request.into()))) };
    let play = |qpath: &str, channel: i32| SoundRequest { qpath: qpath.to_owned(), entity, channel, origin: at_entity };
    // A plain sound_request-mapped sound with edits.
    let simple = |edit: &dyn Fn(&mut PreparedSound)| -> Option<PreparedSoundEvent> {
        Some(sound_request(event, game).map(|request| {
            request.map(|request| {
                let mut sound = PreparedSound::from(request);
                edit(&mut sound);
                sound
            })
        }))
    };

    match event.event {
        E::EV_CLIENTJOIN => {
            // "Force a local reinit of client entity on join": drop stale loops.
            let joined = u16::try_from(event.parm).ok()?;
            Some(Ok(Some(PreparedSound::with_ops(vec![SoundOp::LoopStop { entity: joined }]))))
        }
        E::EV_PAIN => {
            if duel_hides(game, client_num) {
                return none;
            }
            simple(&|sound| sound.gate = SoundGate::Pain)
        }
        E::EV_JUMP => {
            if !own_or_other_allowed(prep.game_sounds.jump, local_client(game) == Some(entity)) {
                return none;
            }
            if duel_hides(game, client_num) {
                return none;
            }
            simple(&|sound| sound.gate = SoundGate::Jump)
        }
        E::EV_TAUNT => {
            if prep.game_sounds.no_taunt || duel_hides(game, client_num) {
                return none;
            }
            simple(&|sound| sound.fallback = Some("*taunt.wav".to_owned()))
        }
        E::EV_DEATH1 | E::EV_DEATH2 | E::EV_DEATH3 => {
            if duel_hides(game, client_num) {
                return none;
            }
            let dramatic = event.parm != 0 && local_client(game) == Some(entity);
            let dramatic_failure = local_sound(game, DRAMATIC_FAILURE, CHAN_LOCAL);
            simple(&|sound| {
                if dramatic {
                    sound.plays.extend(dramatic_failure.clone());
                    sound.ops.push(SoundOp::MusicDip);
                }
            })
        }
        E::EV_FALL | E::EV_ROLL => {
            if local_client(game) == Some(entity) && local_ps(game, "fallingToDeath") != 0 {
                return none;
            }
            if duel_hides(game, client_num) {
                return none;
            }
            let mut plays = Vec::new();
            let mut pain_time = false;
            if event.event == E::EV_FALL || event.parm != 0 {
                // A fall-roll-in-one EV_ROLL also runs DoFall.
                let ski = game.is_japro() && game.japro_cinfo2() & JAPRO_CINFO2_WTTRIBES != 0;
                let (fall, stamps_pain) = fall_sounds(event, ski);
                plays = fall;
                pain_time = stamps_pain;
            }
            let mut alt_qpaths = Vec::new();
            if event.event == E::EV_ROLL {
                if own_or_other_allowed(prep.game_sounds.roll, local_client(game) == Some(entity)) {
                    // A model without its own roll voice reuses its jump voice.
                    plays.push(play("*roll.wav", CHAN_VOICE));
                    alt_qpaths.push(("*roll.wav".to_owned(), "*jump1.wav".to_owned()));
                }
                plays.push(play(ROLL_SOUND, CHAN_BODY));
            }
            let mut sound = PreparedSound::new(plays);
            sound.alt_qpaths = alt_qpaths;
            sound.sets_pain_time = pain_time;
            Some(Ok(Some(sound)))
        }
        E::EV_FOOTSTEP | E::EV_FOOTSTEP_METAL | E::EV_FOOTSPLASH | E::EV_FOOTWADE | E::EV_SWIM => {
            // cg_footsteps 0 silences every footstep event; only EV_FOOTSTEP honours duels.
            if prep.game_sounds.footsteps == 0 || (event.event == E::EV_FOOTSTEP && duel_hides(game, client_num)) {
                return none;
            }
            simple(&|_| {})
        }
        E::EV_FIRE_WEAPON | E::EV_ALT_FIRE => {
            let alt = event.event == E::EV_ALT_FIRE;
            let is_npc = es.field_i32("eType").unwrap_or(0) == ET_NPC;
            let weapon = es.field_i32("weapon").unwrap_or(0);
            // Turret fire is FX only; vehicles do nothing client-side; emplaced
            // guns only make a sound for NPC primary fire.
            if (entity >= MAX_CLIENTS && !is_npc)
                || (is_npc && es.field_i32("NPC_class").unwrap_or(0) == CLASS_VEHICLE)
                || weapon <= 0
                || (weapon == 17 && (alt || !is_npc))
                || duel_hides(game, number)
            {
                return none;
            }
            let sounds = tables::weapon_sounds(weapon);
            match if alt { sounds.alt_flash } else { sounds.flash } {
                Some(qpath) => one(play(qpath, CHAN_WEAPON)),
                None => none,
            }
        }
        E::EV_CHANGE_WEAPON => {
            if duel_hides(game, number) {
                return none;
            }
            let qpath = match tables::weapon_sounds(event.parm).select {
                Some(select) => select,
                None if event.parm != WP_SABER => SELECT_SOUND, // saber has its own turn-on
                None => return none,
            };
            one(play(qpath, CHAN_AUTO))
        }
        E::EV_WEAPON_CHARGE | E::EV_WEAPON_CHARGE_ALT => {
            if duel_hides(game, number) {
                return none;
            }
            let sounds = tables::weapon_sounds(event.parm);
            let qpath = if event.event == E::EV_WEAPON_CHARGE {
                sounds.charge.or((event.parm == WP_DISRUPTOR).then_some(DISRUPTOR_ZOOM_LOOP))
            } else {
                sounds.alt_charge
            };
            match qpath {
                Some(qpath) => one(play(qpath, CHAN_WEAPON)),
                None => none,
            }
        }
        E::EV_ITEM_PICKUP => {
            if local_ps(game, "duelInProgress") != 0 {
                return none;
            }
            let item_entity = event.parm;
            let state = u16::try_from(item_entity).ok().and_then(|n| game.entity_state(n))?;
            let index = state.field_i32("modelindex").unwrap_or(0);
            if index < 1 && state.field_i32("isJediMaster").unwrap_or(0) != 0 {
                return one(play(HOLOCRON_PICKUP_SOUND, CHAN_AUTO)); // a holocron most likely
            }
            let item = jka_movement::bg_item(index);
            let pickup = item.filter(|item| item.item_type != IT_TEAM).and_then(|_| jka_movement::bg_item_pickup_sound(index));
            match pickup {
                Some(qpath) => {
                    let mut sound = PreparedSound::from(play(&qpath, CHAN_AUTO));
                    sound.gate = SoundGate::ItemPickup(item_entity);
                    Some(Ok(Some(sound)))
                }
                None => none,
            }
        }
        E::EV_GLOBAL_ITEM_PICKUP => {
            if local_ps(game, "duelInProgress") != 0 {
                return none;
            }
            match jka_movement::bg_item_pickup_sound(event.parm) {
                Some(qpath) => local_sound(game, &qpath, CHAN_AUTO).and_then(one),
                None => none,
            }
        }
        E::EV_ITEM_POP => one(play(RESPAWN_SOUND, CHAN_AUTO)),
        E::EV_ITEM_RESPAWN => {
            if local_ps(game, "duelInProgress") != 0 {
                return none;
            }
            one(play(RESPAWN_SOUND, CHAN_AUTO))
        }
        use_item if (E::EV_USE_ITEM0.as_i32()..=E::EV_USE_ITEM15.as_i32()).contains(&use_item.as_i32()) => {
            let item_num = use_item.as_i32() - E::EV_USE_ITEM0.as_i32();
            match item_num {
                HI_SEEKER => one(play(DEPLOY_SEEKER_SOUND, CHAN_AUTO)),
                HI_MEDPAC | HI_MEDPAC_BIG => one(play(MEDKIT_SOUND, CHAN_AUTO)),
                HI_BINOCULARS => {
                    // CG_ToggleBinoculars: only the viewer's own, weapon-ready toggle.
                    if local_client(game) != Some(entity) || local_ps(game, "weaponstate") != PS_WEAPON_READY {
                        return none;
                    }
                    let zoom = match event.parm {
                        2 => 0,
                        1 => 2,
                        _ => local_ps(game, "zoomMode"),
                    };
                    match zoom {
                        0 => local_sound(game, ZOOM_START_SOUND, CHAN_AUTO).and_then(one),
                        2 => local_sound(game, ZOOM_END_SOUND, CHAN_AUTO).and_then(one),
                        _ => none,
                    }
                }
                _ => none,
            }
        }
        E::EV_TEAM_POWER => {
            let qpath = if event.parm == 1 { TEAM_HEAL_SOUND } else { TEAM_REGEN_SOUND };
            let plays: Vec<SoundRequest> = (0..i32::from(MAX_CLIENTS))
                .filter(|&client| in_client_bitflags(es, client))
                .map(|client| SoundRequest {
                    qpath: qpath.to_owned(),
                    entity: client as u16,
                    channel: CHAN_AUTO,
                    origin: at_entity,
                })
                .collect();
            if plays.is_empty() {
                return none;
            }
            Some(Ok(Some(PreparedSound::new(plays))))
        }
        E::EV_FORCE_DRAINED => {
            let owner = u16::try_from(es.field_i32("owner").unwrap_or(-1)).ok()?;
            one(SoundRequest { qpath: DRAIN_SOUND.to_owned(), entity: owner, channel: CHAN_AUTO, origin: at_entity })
        }
        E::EV_GIB_PLAYER => {
            if duel_hides(game, number) {
                return none;
            }
            if prep.game_sounds.blood > 0 {
                one(play(GIB_SOUND, CHAN_BODY))
            } else if es.field_i32("eFlags").unwrap_or(0) & EF_DEAD == 0 {
                // cg_blood 0: a living victim just yells.
                let death = format!("*death{}.wav", event.receive_sequence % 3 + 1);
                one(play(&death, CHAN_VOICE))
            } else {
                none
            }
        }
        E::EV_PLAYER_TELEPORT_IN | E::EV_PLAYER_TELEPORT_OUT => {
            if duel_hides(game, client_num) {
                return none;
            }
            let qpath = if event.event == E::EV_PLAYER_TELEPORT_IN { TELE_IN_SOUND } else { TELE_OUT_SOUND };
            one(play(qpath, CHAN_AUTO))
        }
        E::EV_OBITUARY => {
            // jaPRO cg_killSounds: your own kill (not a suicide).
            let target = es.field_i32("otherEntityNum").unwrap_or(-1);
            let attacker = es.field_i32("otherEntityNum2").unwrap_or(-1);
            let level = prep.game_sounds.kill;
            if level == 0 || attacker == target || local_client(game).map(i32::from) != Some(attacker) {
                return none;
            }
            // MOD_SABER, MOD_BOWCASTER, MOD_REPEATER_ALT, MOD_ROCKET, MOD_CONC.
            let airborne_weapon = matches!(event.parm, 3 | 11 | 13 | 19 | 29);
            let airborne = u16::try_from(target)
                .ok()
                .and_then(|number| game.entity_state(number))
                .is_some_and(|state| state.field_i32("groundEntityNum").unwrap_or(0) == ENTITYNUM_NONE);
            let qpath = if level > 1 && airborne && airborne_weapon { FRAG_MIDAIR_SOUND } else { FRAG_SOUND };
            local_sound(game, qpath, CHAN_LOCAL).and_then(one)
        }
        E::EV_NOAMMO => {
            // A saber "out of ammo" flashes the force HUD with the noforce sound.
            if local_client(game) == Some(entity)
                && local_ps(game, "weapon") == WP_SABER
                && !duel_hides(game, number)
            {
                local_sound(game, "sound/weapons/force/noforce", CHAN_LOCAL).and_then(one)
            } else {
                none
            }
        }
        E::EV_GENERAL_SOUND
            if game.is_japro()
                && es.field_i32("eFlags2").unwrap_or(0) & RS_TIMER_START != 0
                && prep.game_sounds.race_sounds & RS_TIMER_START as u8 == 0 =>
        {
            none // cg_raceSounds: the race start-trigger sound is switched off
        }
        E::EV_DEBRIS => {
            // CG_Chunks' breaking sound, chosen by material_t (`trickedentindex`).
            let owner = u16::try_from(es.field_i32("owner").unwrap_or(-1)).unwrap_or(entity);
            let pick = event.receive_sequence as usize;
            let spark;
            let qpath: &str = match es.field_i32("trickedentindex").unwrap_or(8) {
                1 | 6 => "sound/weapons/explosions/glassbreak1",
                12 => "sound/effects/grate_destroy",
                2 => {
                    spark = format!("sound/ambience/spark{}.wav", pick % 6 + 1);
                    &spark
                }
                4 | 5 | 9 | 15 | 16 => "sound/effects/wall_smash",
                11 | 14 => ["sound/weapons/explosions/crateBust1", "sound/weapons/explosions/crateBust2"][pick % 2],
                0 | 3 | 7 | 10 => "sound/weapons/explosions/glasslcar",
                _ => return none, // MAT_NONE, MAT_ROPE
            };
            let mut sound = PreparedSound::from(SoundRequest {
                qpath: qpath.to_owned(),
                entity: owner,
                channel: CHAN_BODY,
                origin: SoundOrigin::Fixed(super::entity_vec3(es, "origin").unwrap_or(event.position)),
            });
            sound.result = EventDispatchResult::Handled("SOUND_PLAYED");
            Some(Ok(Some(sound)))
        }
        E::EV_GLASS_SHATTER => {
            // CG_DoGlass: glassbreak1 at the damage point.
            let origin = super::entity_vec3(es, "origin").unwrap_or(event.position);
            let mut sound = PreparedSound::from(SoundRequest {
                qpath: "sound/effects/glassbreak1.wav".to_owned(),
                entity,
                channel: CHAN_AUTO,
                origin: SoundOrigin::Fixed(origin),
            });
            sound.result = EventDispatchResult::Handled("SOUND_PLAYED");
            Some(Ok(Some(sound)))
        }
        E::EV_DISRUPTOR_ZOOMSOUND => {
            if duel_hides(game, client_num) || local_client(game) != Some(entity) {
                return none;
            }
            let qpath = if local_ps(game, "zoomMode") != 0 { DISRUPTOR_ZOOM_START } else { DISRUPTOR_ZOOM_END };
            local_sound(game, qpath, CHAN_AUTO).and_then(one)
        }
        E::EV_BECOME_JEDIMASTER => {
            let plays = vec![play(JEDI_MASTER_SABER_ON, CHAN_AUTO)];
            let mut sound = PreparedSound::new(plays);
            if local_client(game) == Some(entity) {
                sound.plays.extend(local_sound(game, HAPPY_MUSIC, CHAN_LOCAL));
                sound.ops.push(SoundOp::MusicDip);
            }
            Some(Ok(Some(sound)))
        }
        E::EV_GLOBAL_DUEL => {
            let local = i32::from(local_client(game)?);
            let involved = ["otherEntityNum", "otherEntityNum2", "groundEntityNum"]
                .iter()
                .any(|field| es.field_i32(field) == Some(local));
            if involved {
                local_sound(game, COUNT_FIGHT_SOUND, CHAN_ANNOUNCER).and_then(one)
            } else {
                none
            }
        }
        E::EV_PRIVATE_DUEL => {
            if local_client(game) != Some(entity) {
                return none;
            }
            if event.parm == 0 {
                // The duel ended: the level music returns.
                return Some(Ok(Some(PreparedSound::with_ops(vec![SoundOp::MapMusic]))));
            }
            if prep.game_sounds.duel != 0 && event.parm == 2 && !game.japro_racemode() {
                // Duel begin counts down (the BEGIN DUEL text is not drawn yet).
                if matches!(prep.game_sounds.duel, 1 | 2) {
                    local_sound(game, COUNT_FIGHT_SOUND, CHAN_ANNOUNCER).and_then(one)
                } else {
                    none
                }
            } else if prep.game_sounds.duel_music {
                Some(Ok(Some(PreparedSound::with_ops(vec![SoundOp::DuelMusic]))))
            } else {
                none
            }
        }
        E::EV_GLOBAL_TEAM_SOUND => {
            match tables::global_team_sound(event.parm, game.gametype() == GT_CTY) {
                Some(qpath) => local_sound(game, qpath, CHAN_ANNOUNCER).map(|request| {
                    let mut sound = PreparedSound::from(request);
                    sound.buffered = true;
                    Ok(Some(sound))
                }),
                None => none,
            }
        }
        E::EV_PREDEFSOUND if event.parm == 3 => simple(&|sound| {
            // Absorb-hit also needs a visual team-power effect.
            sound.result = EventDispatchResult::Handled("SOUND_PLAYED_ABSORB_SHELL");
        }),
        E::EV_VOICECMD_SOUND => {
            let voice_client = u16::try_from(es.field_i32("groundEntityNum").unwrap_or(-1)).ok();
            // Teammates also hear the line as a radio copy at the listener's head.
            let mirror = local_client(game).zip(voice_client).and_then(|(local, speaker)| {
                let source = game.client_info(usize::from(speaker), siege_classes)?;
                let listener = game.client_info(usize::from(local), siege_classes)?;
                (local != speaker && source.team == listener.team).then(|| SoundRequest {
                    qpath: String::new(),
                    entity: local,
                    channel: CHAN_MENU1, // radio/head copy
                    origin: SoundOrigin::Local,
                })
            });
            simple(&|sound| {
                sound.mirror = mirror.clone().map(|mut radio| {
                    radio.qpath = sound.plays[0].qpath.clone();
                    radio
                });
                sound.result = EventDispatchResult::Partial("VOICE_SOUND_PLAYED_CHAT_TEXT_PENDING");
            })
        }
        E::EV_GENERAL_SOUND => {
            let channel = es.field_i32("saberEntityNum").unwrap_or(0);
            if !matches!(channel, TRACK_CHANNEL_2 | TRACK_CHANNEL_3 | TRACK_CHANNEL_5) {
                return None; // ordinary one-shot
            }
            // Force speed/rage/sight: a real looping sound tracked on the entity.
            let Some(qpath) = game.sound_qpath(event.parm) else { return Some(Err("SOUND_RESOURCE_MISSING")) };
            Some(Ok(Some(PreparedSound::with_ops(vec![SoundOp::LoopStart {
                entity,
                request: SoundRequest { qpath, entity, channel, origin: at_entity },
            }]))))
        }
        E::EV_STARTLOOPINGSOUND => {
            let Some(qpath) = game.sound_qpath(event.parm) else { return Some(Err("SOUND_RESOURCE_MISSING")) };
            Some(Ok(Some(PreparedSound::with_ops(vec![SoundOp::LoopStart {
                entity,
                request: SoundRequest { qpath, entity, channel: CHAN_AUTO, origin: at_entity },
            }]))))
        }
        E::EV_STOPLOOPINGSOUND => Some(Ok(Some(PreparedSound::with_ops(vec![SoundOp::LoopStop { entity }])))),
        E::EV_MUTE_SOUND => {
            let muted = u16::try_from(es.field_i32("trickedentindex2").unwrap_or(-1)).ok()?;
            let channel = es.field_i32("trickedentindex").unwrap_or(-1);
            Some(Ok(Some(PreparedSound::with_ops(vec![
                SoundOp::Mute { entity: muted, channel },
                SoundOp::LoopStop { entity: muted },
            ]))))
        }
        E::EV_BMODEL_SOUND => match bmodel_stage_sound(es, game, prep, event.parm) {
            Some(qpath) => one(play(&qpath, CHAN_AUTO)),
            None => none,
        },
        E::EV_PLAYDOORSOUND => match bmodel_stage_sound(es, game, prep, event.parm) {
            Some(qpath) => one(play(&qpath, CHAN_AUTO)),
            None => none,
        },
        E::EV_PLAYDOORLOOPSOUND => match bmodel_stage_sound(es, game, prep, BMS_MID) {
            Some(qpath) => Some(Ok(Some(PreparedSound::with_ops(vec![SoundOp::LoopStart {
                entity,
                request: SoundRequest { qpath, entity, channel: CHAN_AUTO, origin: at_entity },
            }])))),
            None => none,
        },
        _ => None,
    }
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
/// `cent->lerpOrigin`, shifted to the inline-model midpoint for brush models
/// (doors and other movers have a "crazy origin", per rww).
fn loop_origin(entity: &PresentedEntity, midpoints: &[[f32; 3]]) -> [f32; 3] {
    if entity.state.field_i32("solid").unwrap_or(0) == SOLID_BMODEL {
        let midpoint = usize::try_from(entity.state.field_i32("modelindex").unwrap_or(0))
            .ok()
            .and_then(|index| midpoints.get(index))
            .copied()
            .unwrap_or([0.0; 3]);
        std::array::from_fn(|i| entity.origin[i] + midpoint[i])
    } else {
        entity.origin
    }
}

fn entity_loop(
    entity: &PresentedEntity,
    game: &ClientGameState,
    ambient: &AmbientSets,
    midpoints: &[[f32; 3]],
) -> Option<LoopRequest> {
    let state = &entity.state;
    let loop_sound = state.field_i32("loopSound").unwrap_or(0);
    let soundset = state.field_i32("loopIsSoundset").unwrap_or(0) != 0 && entity.number >= MAX_CLIENTS;
    if loop_sound == 0 && !soundset { return None; }
    let qpath = if soundset {
        // loopSound selects BMS_START/MID/END of CS_AMBIENT_SET + soundSetIndex.
        let index = u16::try_from(state.field_i32("soundSetIndex").unwrap_or(0)).ok()?;
        let set = game.configstring(CS_AMBIENT_SET.checked_add(index)?)?;
        let set = String::from_utf8_lossy(set);
        ambient.bmodel_sound(set.trim(), loop_sound)?.to_owned()
    } else {
        let qpath = game.sound_qpath(loop_sound)?;
        if qpath.starts_with('*') { return None; } // custom player sounds never loop
        qpath
    };
    Some(LoopRequest { qpath, origin: SoundOrigin::Fixed(loop_origin(entity, midpoints)), volume: 1.0 })
}

/// CG_Missile "add missile sound": the weapon's (alt) missile hum follows the missile.
fn missile_loop(entity: &PresentedEntity) -> Option<LoopRequest> {
    let state = &entity.state;
    let weapon = state.field_i32("weapon").unwrap_or(0);
    let sounds = tables::weapon_sounds(weapon);
    let alt = state.field_i32("eFlags").unwrap_or(0) & EF_ALT_FIRING != 0;
    let qpath = if alt { sounds.alt_missile } else { sounds.missile }?;
    Some(LoopRequest { qpath: qpath.to_owned(), origin: SoundOrigin::Fixed(entity.origin), volume: 1.0 })
}
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
        out.push(LoopRequest { qpath: qpath.clone(), origin, volume: 1.0 });
        first = Some(qpath);
    }
    // A dual saber's second blade is off at saberHolstered 1.
    if !info.saber2_name.is_empty() && !saber_name_is_removed(&info.saber2_name) && holstered == 0 {
        let qpath = sound_loop(&info.saber2_name);
        if first.as_deref() != Some(qpath.as_str()) {
            out.push(LoopRequest { qpath, origin, volume: 1.0 });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jka_protocol::gamestate::{EntityState, ENTITY_FIELDS};

    fn event(event: EntityEvent, parm: i32) -> PresentationEvent {
        let mut state = EntityState { number: 3, fields: [0; ENTITY_FIELDS.len()] };
        if let Some(index) = ENTITY_FIELDS.iter().position(|(name, _)| *name == "eType") {
            state.fields[index] = ET_PLAYER as u32;
        }
        PresentationEvent {
            receive_sequence: 0,
            source_entity_num: 3,
            entity_num: 3,
            event,
            raw_event: event.as_i32(),
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

        let jump = sound_request(&event(EntityEvent::EV_JUMP, 0), &game).unwrap().unwrap();
        assert_eq!(jump.qpath, "*jump1.wav");
        assert_eq!(jump.channel, 3); // CHAN_VOICE

        let hard_fall = sound_request(&event(EntityEvent::EV_FALL, 50), &game).unwrap().unwrap();
        assert_eq!(hard_fall.qpath, "sound/player/fallsplat.wav");
        assert_eq!(hard_fall.channel, 0); // CHAN_AUTO

        let roll = sound_request(&event(EntityEvent::EV_ROLL, 0), &game).unwrap().unwrap();
        assert_eq!(roll.qpath, "sound/player/roll1.wav");
        assert_eq!(roll.channel, 6); // CHAN_BODY
    }

    #[test]
    fn voice_events_map_to_custom_voice_sounds_on_chan_voice() {
        let game = ClientGameState::new();
        let cases = [
            (EntityEvent::EV_PAIN, 10, "*pain25.wav"),
            (EntityEvent::EV_PAIN, 30, "*pain50.wav"),
            (EntityEvent::EV_PAIN, 60, "*pain75.wav"),
            (EntityEvent::EV_PAIN, 100, "*pain100.wav"),
            (EntityEvent::EV_DEATH1, 0, "*death1.wav"),
            (EntityEvent::EV_DEATH3, 0, "*death3.wav"),
            (EntityEvent::EV_PUSHED2, 0, "*pushed2.wav"),
            (EntityEvent::EV_CHOKE3, 0, "*choke3.wav"),
            (EntityEvent::EV_COVER5, 0, "*cover5.wav"),
            (EntityEvent::EV_GIVEUP4, 0, "*giveup4.wav"),
            (EntityEvent::EV_LOST1, 0, "*lost1.wav"),
            (EntityEvent::EV_OUTFLANK2, 0, "*outflank2.wav"),
            (EntityEvent::EV_GLOAT3, 0, "*gloat3.wav"),
            (EntityEvent::EV_PUSHFAIL, 0, "*pushfail.wav"),
        ];
        for (ev, parm, expected) in cases {
            let request = sound_request(&event(ev, parm), &game).unwrap().unwrap();
            assert_eq!(request.qpath, expected, "{ev:?}");
            assert_eq!(request.channel, 3, "{ev:?} must use CHAN_VOICE");
        }
        let taunt = sound_request(&event(EntityEvent::EV_TAUNT, 0), &game).unwrap().unwrap();
        assert!(taunt.qpath.starts_with('*') && taunt.qpath.ends_with(".wav"));
    }

    fn set_field(event: &mut PresentationEvent, name: &str, value: i32) {
        let index = ENTITY_FIELDS.iter().position(|(field, _)| *field == name).unwrap();
        event.state.fields[index] = value as u32;
    }

    fn prepared(event: &PresentationEvent) -> PreparedSound {
        let game = ClientGameState::new();
        prepare_sound_event(event, &game, &[], &SoundPrepData::default())
            .unwrap()
            .expect("event should prepare a sound")
    }

    #[test]
    fn weapon_events_use_register_weapon_sounds() {
        let mut fire = event(EntityEvent::EV_FIRE_WEAPON, 0);
        set_field(&mut fire, "weapon", WP_BLASTER);
        let sound = prepared(&fire);
        assert_eq!(sound.plays[0].qpath, "sound/weapons/blaster/fire.wav");
        assert_eq!(sound.plays[0].channel, CHAN_WEAPON);

        let mut alt = event(EntityEvent::EV_ALT_FIRE, 0);
        set_field(&mut alt, "weapon", WP_BOWCASTER);
        assert_eq!(prepared(&alt).plays[0].qpath, "sound/weapons/bowcaster/fire.wav");

        // Saber fire is silent; switching to it plays no select sound (it has its own turn-on).
        let game = ClientGameState::new();
        let mut saber = event(EntityEvent::EV_FIRE_WEAPON, 0);
        set_field(&mut saber, "weapon", WP_SABER);
        let none = prepare_sound_event(&saber, &game, &[], &SoundPrepData::default()).unwrap();
        assert!(none.is_none());
        let change_saber = event(EntityEvent::EV_CHANGE_WEAPON, WP_SABER);
        assert!(prepare_sound_event(&change_saber, &game, &[], &SoundPrepData::default()).unwrap().is_none());
        assert_eq!(prepared(&event(EntityEvent::EV_CHANGE_WEAPON, WP_BLASTER)).plays[0].qpath, "sound/weapons/blaster/select.wav");
        assert_eq!(prepared(&event(EntityEvent::EV_CHANGE_WEAPON, WP_MELEE)).plays[0].qpath, SELECT_SOUND);

        assert_eq!(prepared(&event(EntityEvent::EV_WEAPON_CHARGE, WP_BOWCASTER)).plays[0].qpath, "sound/weapons/bowcaster/altcharge.wav");
        assert_eq!(prepared(&event(EntityEvent::EV_WEAPON_CHARGE, WP_DISRUPTOR)).plays[0].qpath, DISRUPTOR_ZOOM_LOOP);
        assert_eq!(prepared(&event(EntityEvent::EV_WEAPON_CHARGE_ALT, WP_DEMP2)).plays[0].qpath, "sound/weapons/demp2/altCharge.wav");
    }

    #[test]
    fn falls_and_rolls_layer_voice_and_body_sounds() {
        let hard = prepared(&event(EntityEvent::EV_FALL, 60));
        assert_eq!(hard.plays[0].qpath, FALL_SOUND);
        assert_eq!(hard.plays[1].qpath, "*land1.wav");
        assert!(hard.sets_pain_time);

        let soft = prepared(&event(EntityEvent::EV_FALL, 10));
        assert_eq!(soft.plays.len(), 1);
        assert_eq!(soft.plays[0].qpath, LAND_SOUND);
        assert!(!soft.sets_pain_time);

        let roll = prepared(&event(EntityEvent::EV_ROLL, 0));
        assert_eq!(roll.plays[0].qpath, "*roll.wav");
        assert_eq!(roll.alt_qpaths, [("*roll.wav".to_owned(), "*jump1.wav".to_owned())]);
        assert_eq!(roll.plays[1].qpath, ROLL_SOUND);
        let fall_roll = prepared(&event(EntityEvent::EV_ROLL, 60));
        assert_eq!(fall_roll.plays[0].qpath, FALL_SOUND);
        assert_eq!(fall_roll.plays.last().unwrap().qpath, ROLL_SOUND);

        let mut corpse = event(EntityEvent::EV_FALL, 30);
        set_field(&mut corpse, "eFlags", EF_DEAD);
        assert_eq!(prepared(&corpse).plays[0].qpath, FALL_SOUND);
    }

    #[test]
    fn pain_jump_and_pickup_events_are_gated_for_dispatch() {
        assert_eq!(prepared(&event(EntityEvent::EV_PAIN, 10)).gate, SoundGate::Pain);
        assert_eq!(prepared(&event(EntityEvent::EV_JUMP, 0)).gate, SoundGate::Jump);
        let taunt = prepared(&event(EntityEvent::EV_TAUNT, 0));
        assert_eq!(taunt.fallback.as_deref(), Some("*taunt.wav"));
    }

    #[test]
    fn loop_and_mute_events_produce_persistent_ops() {
        let stop = prepared(&event(EntityEvent::EV_STOPLOOPINGSOUND, 0));
        assert!(stop.plays.is_empty());
        assert!(matches!(stop.ops[0], SoundOp::LoopStop { entity: 3 }));

        let mut mute = event(EntityEvent::EV_MUTE_SOUND, 0);
        set_field(&mut mute, "trickedentindex2", 7);
        set_field(&mut mute, "trickedentindex", 52);
        let sound = prepared(&mute);
        assert!(matches!(sound.ops[0], SoundOp::Mute { entity: 7, channel: 52 }));
        assert!(matches!(sound.ops[1], SoundOp::LoopStop { entity: 7 }));
    }

    #[test]
    fn client_bitflags_span_all_four_words() {
        let mut ev = event(EntityEvent::EV_TEAM_POWER, 1);
        set_field(&mut ev, "trickedentindex", 1 << 3);
        set_field(&mut ev, "trickedentindex2", 1 << 1);
        set_field(&mut ev, "trickedentindex3", 1 << 1);
        assert!(in_client_bitflags(&ev.state, 3));
        assert!(in_client_bitflags(&ev.state, 33));
        assert!(!in_client_bitflags(&ev.state, 4));
        let sound = prepared(&ev);
        // Only the first 32 clients are audible (MAX_CLIENTS); the higher words exist for the wire format.
        assert_eq!(sound.plays.iter().map(|p| p.entity).collect::<Vec<_>>(), vec![3, 17]);
    }

    /// Every hard-coded sound path must exist in the stock assets. Run with
    /// `JKA_TEST_BASE=<base> cargo test -p DinurdoJK stock_sound_tables -- --ignored`.
    #[test]
    #[ignore = "needs JKA_TEST_BASE"]
    fn stock_sound_tables_resolve_in_stock_assets() {
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let mut assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let ambient = load_ambient_sets(&mut SoundAssets::new(AssetSearchPath::open(std::path::Path::new(&base)).unwrap()));
        let mut cache = SoundAssets::new(AssetSearchPath::open(std::path::Path::new(&base)).unwrap());
        let _ = &mut assets;
        let mut paths: Vec<String> = Vec::new();
        for weapon in 1..=18 {
            let w = tables::weapon_sounds(weapon);
            for q in [w.select, w.flash, w.alt_flash, w.charge, w.alt_charge, w.missile, w.alt_missile].into_iter().flatten() {
                paths.push(q.to_owned());
            }
        }
        paths.extend(
            [
                SELECT_SOUND, FALL_SOUND, LAND_SOUND, OBJECT_HIT_SOUND, ROLL_SOUND, GIB_SOUND, TELE_IN_SOUND,
                TELE_OUT_SOUND, RESPAWN_SOUND, HOLOCRON_PICKUP_SOUND, ZOOM_START_SOUND, ZOOM_END_SOUND,
                DISRUPTOR_ZOOM_START, DISRUPTOR_ZOOM_END, DISRUPTOR_ZOOM_LOOP, DEPLOY_SEEKER_SOUND, MEDKIT_SOUND,
                TEAM_HEAL_SOUND, TEAM_REGEN_SOUND, DRAIN_SOUND, COUNT_FIGHT_SOUND, JEDI_MASTER_SABER_ON,
                HAPPY_MUSIC, DRAMATIC_FAILURE,
            ]
            .map(str::to_owned),
        );
        for index in 1..80 {
            if let Some(q) = jka_movement::bg_item_pickup_sound(index) {
                paths.push(q);
            }
        }
        for parm in 1..=6 {
            paths.push(tables::predefined_sound(parm).unwrap().to_owned());
        }
        // Character voices (bare-FFFB MP3s with a Xing header that gapless decoding trimmed to nothing).
        for name in ["pain25", "pain100", "death1", "jump1", "land1", "taunt", "gasp", "anger1", "pushed1", "choke1"] {
            paths.push(format!("sound/chars/mp_generic_male/misc/{name}"));
            paths.push(format!("sound/chars/kyle/misc/{name}"));
        }
        for parm in 2..=10 {
            paths.extend(tables::global_team_sound(parm, false).map(str::to_owned));
        }
        paths.sort();
        paths.dedup();
        let missing: Vec<_> = paths
            .iter()
            .filter_map(|q| cache.register(q).err().map(|error| format!("{q} ({error})")))
            .collect();
        let door = ambient.bmodel_sound("impdoor1", BMS_MID).expect("sound.txt bmodelSet impdoor1");
        assert_eq!(door, "sound/movers/doors/door1move.wav");
        assert!(cache.register(door).is_ok(), "door set sound must decode");
        println!("checked {} sound paths", paths.len());
        // Sounds stock JKA itself never shipped (the engine plays silence for them too).
        const ABSENT_IN_STOCK: [&str; 7] = [
            "sound/player/gibsplt1.wav",
            "sound/weapons/rocket/rocklf1a.wav",
            "misc/gasp",
            "misc/anger1",
            "misc/pushed1",
            "sound/weapons/bowcaster/altfire.wav",
            "protocol/misc/40MOM",
        ];
        let unexpected: Vec<_> = missing.iter().filter(|q| !ABSENT_IN_STOCK.iter().any(|a| q.contains(a))).collect();
        assert!(unexpected.is_empty(), "missing stock sounds: {unexpected:?} (all missing: {missing:?})");
    }

    #[test]
    fn footstep_material_selects_openjk_pool() {
        let game = ClientGameState::new();
        let metal = sound_request(&event(EntityEvent::EV_FOOTSTEP, 3), &game).unwrap().unwrap();
        assert!(metal.qpath.starts_with("sound/player/footsteps/metal_step"));
        assert!(metal.qpath.ends_with(".wav"));
        assert_eq!(metal.channel, 6);

        let wood = sound_request(&event(EntityEvent::EV_FOOTSTEP, 1), &game).unwrap().unwrap();
        assert!(wood.qpath.starts_with("sound/player/footsteps/wood_walk"));
        assert_eq!(wood.channel, 6);
    }

    #[test]
    fn event_variant_is_stable_for_same_event() {
        let event = event(EntityEvent::EV_SABER_ATTACK, 7);
        assert_eq!(event_variant(&event, 8), event_variant(&event, 8));
        assert!(event_variant(&event, 8) < 8);
    }

    #[test]
    fn japro_voice_options_gate_jump_roll_and_taunt() {
        let with = |edit: &dyn Fn(&mut crate::config::GameOptions), event: &PresentationEvent| {
            let mut prep = SoundPrepData::default();
            edit(&mut prep.game_sounds);
            prepare_sound_event(event, &ClientGameState::new(), &[], &prep).unwrap()
        };
        // No viewer snapshot, so every mover counts as "someone else".
        let jump = event(EntityEvent::EV_JUMP, 0);
        assert!(with(&|o| o.jump = 0, &jump).is_none());
        assert!(with(&|o| o.jump = 1, &jump).is_some());
        assert!(with(&|o| o.jump = 2, &jump).is_some());
        assert!(with(&|o| o.jump = 3, &jump).is_none());

        let roll = event(EntityEvent::EV_ROLL, 0);
        let plays = |o: u8| with(&|s| s.roll = o, &roll).unwrap().plays.len();
        assert_eq!(plays(0), 1, "only the body roll sound remains");
        assert_eq!(plays(1), 2);
        assert_eq!(plays(3), 1);

        let taunt = event(EntityEvent::EV_TAUNT, 0);
        assert!(with(&|o| o.no_taunt = true, &taunt).is_none());
        assert!(with(&|o| o.no_taunt = false, &taunt).is_some());
    }
}

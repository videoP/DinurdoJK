//! Permanent wire identities and independent, bounded remote playback.
use super::*;
use crate::emote_net::RemoteEmoteStart;
use jka_protocol::server::Snapshot;
use std::{collections::HashMap, sync::mpsc};

pub const STOP_ID: u16 = crate::emote_net_codec::EMOTE_CAPACITY - 1;
const MAX_CLIENTS: usize = 32;

struct Registry {
    by_key: HashMap<&'static str, u16>,
    by_id: HashMap<u16, &'static str>,
}

fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut by_key = HashMap::new();
        let mut by_id = HashMap::new();
        for line in include_str!("../emote_network_ids.tsv")
            .lines()
            .filter(|l| !l.starts_with('#') && !l.is_empty())
        {
            let (id, key) = line.split_once('\t').expect("emote registry row");
            let id: u16 = id.parse().expect("emote registry ID");
            assert!(id < crate::emote_net_codec::MATCH_BASE_ID && key.split('\t').count() == 2,
                "registry must not allocate reserved MATCH or STOP IDs");
            assert!(by_key.insert(key, id).is_none(), "duplicate emote identity");
            assert!(by_id.insert(id, key).is_none(), "duplicate emote wire ID");
        }
        Registry { by_key, by_id }
    })
}

pub fn wire_id(emote: &Emote) -> Option<u16> {
    registry().by_key.get(emote.network_key.as_str()).copied()
}

pub fn resolve(catalog: &[Emote], id: u16) -> Option<Emote> {
    let key = *registry().by_id.get(&id)?;
    catalog
        .iter()
        .flat_map(|e| std::iter::once(e).chain(&e.variants))
        .find(|e| e.network_key == key)
        .cloned()
}

struct Remote {
    generation: u64,
    // Resolved identity and absolute timeline start. A MATCH captures these at
    // reception; if the referenced dancer later leaves or stops, we continue.
    wire_id: u16,
    matched: bool,
    start: i32,
    flags: i32,
    taunt: taunt::Taunt,
    state: PlaybackState,
}

struct Job {
    client: usize,
    generation: u64,
    emote: Emote,
}
type Prepared = (
    usize,
    u64,
    Result<(Emote, Vec<crate::audio::SoundDecodeResult>), String>,
);

struct Runtime {
    own_client: Option<i32>,
    generation: u64,
    slots: [Option<Remote>; MAX_CLIENTS],
    stop_pending: [bool; MAX_CLIENTS],
    render_time: i32,
    jobs: mpsc::SyncSender<Job>,
    prepared: mpsc::Receiver<Prepared>,
}

impl Runtime {
    fn cancel(&mut self, client: usize) {
        if self.slots[client].take().is_some() {
            self.stop_pending[client] = true;
        }
    }

    fn reset(&mut self) {
        self.own_client = None;
        for client in 0..MAX_CLIENTS {
            self.cancel(client);
        }
        // Generations never reset: queued worker results cannot resurrect a session.
    }
}

fn runtime() -> &'static Mutex<Runtime> {
    static RUNTIME: OnceLock<Mutex<Runtime>> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        let (jobs, incoming) = mpsc::sync_channel::<Job>(MAX_CLIENTS);
        let (completed, prepared) = mpsc::sync_channel(MAX_CLIENTS);
        std::thread::Builder::new()
            .name("remote-emote-assets".into())
            .spawn(move || {
                while let Ok(job) = incoming.recv() {
                    let current = runtime().lock().unwrap().slots[job.client]
                        .as_ref()
                        .is_some_and(|s| s.generation == job.generation);
                    if !current {
                        continue;
                    }
                    let result = prepare(job.emote);
                    if completed
                        .send((job.client, job.generation, result))
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .expect("remote emote worker");
        Mutex::new(Runtime {
            own_client: None,
            generation: 0,
            slots: std::array::from_fn(|_| None),
            stop_pending: [false; MAX_CLIENTS],
            render_time: 0,
            jobs,
            prepared,
        })
    })
}

fn prepare(mut emote: Emote) -> Result<(Emote, Vec<crate::audio::SoundDecodeResult>), String> {
    if emote.gla.starts_with("emotes/.bundles/") {
        (emote, _) = prepare_visuals(emote)?;
    }
    let mut audio = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // Prefer world tracks; old bundles with only preview audio still work.
    for lane in [AudioLane::Music, AudioLane::Foley] {
        if !emote
            .audio
            .iter()
            .any(|a| a.lane == lane && a.wav.is_some() && a.audience != AudioAudience::Preview)
        {
            for cue in emote.audio.iter_mut().filter(|a| a.lane == lane) {
                cue.audience = AudioAudience::Both;
            }
        }
    }
    for cue in emote
        .audio
        .iter()
        .filter(|a| a.audience != AudioAudience::Preview)
    {
        if let Some(path) = cue.wav.as_ref().filter(|p| seen.insert((*p).clone())) {
            if let Some(bytes) = read_asset(path, jka_emotes::MAX_WAV_BYTES)? {
                audio.push(crate::audio::SoundDecodeJob::from_emote(path.clone(), bytes).decode());
            }
        }
    }
    Ok((emote, audio))
}

pub fn reset() {
    runtime().lock().unwrap().reset();
}

/// Snapshot-only invalidation; rendering never guesses IDs from interpolated angles.
pub fn on_snapshot(
    snapshot: &Snapshot,
    own_client: i32,
    starts: &[RemoteEmoteStart],
    catalog: &[Emote],
) {
    let mut rt = runtime().lock().unwrap();
    update(&mut rt, snapshot, own_client, starts, catalog);
}

fn update(
    rt: &mut Runtime,
    snapshot: &Snapshot,
    own_client: i32,
    starts: &[RemoteEmoteStart],
    catalog: &[Emote],
) {
    rt.own_client = Some(own_client);
    for client in 0..MAX_CLIENTS {
        let Some(active) = &mut rt.slots[client] else {
            continue;
        };
        let entity = snapshot.entities.iter().find(|e| {
            e.number as usize == client
                && e.field_i32("eType") == Some(1)
                && e.field_i32("clientNum") == Some(client as i32)
        });
        let cancel = entity.is_none_or(|es| {
            let flags = es.field_i32("eFlags").unwrap_or(0);
            flags & ((1 << 1) | (1 << 9) | (1 << 10)) != 0 // dead / firing
                || (flags ^ active.flags) & (1 << 3) != 0 // teleport / respawn
                || active.taunt.interrupted(
                    es.field_i32("torsoAnim").unwrap_or(-1),
                    es.field_i32("legsAnim").unwrap_or(-1),
                )
        });
        if cancel {
            rt.cancel(client);
        }
    }
    // Invalidate every affected slot before resolving new starts, so a MATCH
    // cannot silently capture an emote that was stopped in this snapshot.
    for start in starts {
        if start.client_num < MAX_CLIENTS && start.client_num as i32 != own_client {
            rt.cancel(start.client_num);
        }
    }
    // Ordinary IDs before references, irrespective of entity iteration order.
    // Resolve MATCH chains in further passes (up to the 32 possible clients).
    let mut pending: Vec<_> = starts.iter()
        .filter(|s| s.client_num < MAX_CLIENTS && s.client_num as i32 != own_client
            && s.emote_id != STOP_ID)
        .copied()
        .collect();
    for _ in 0..MAX_CLIENTS {
        if pending.is_empty() { break; }
        let mut progressed = false;
        pending.retain(|start| {
            let is_match = crate::emote_net_codec::match_source_client(start.emote_id).is_some();
            let resolved = match crate::emote_net_codec::match_source_client(start.emote_id) {
                None => Some((start.emote_id, start.server_time)),
                Some(source) if source as usize == start.client_num => None,
                Some(source) if i32::from(source) == own_client => {
                    // The source's own client does not keep a Remote slot for
                    // itself; read the live local playback to resolve that case.
                    active_preview().and_then(|sample| {
                        let wire_id = wire_id(&sample.emote)?;
                        Some((wire_id, start.server_time.saturating_sub(sample.elapsed_ms)))
                    })
                }
                Some(source) => rt.slots[usize::from(source)]
                    .as_ref()
                    .map(|remote| (remote.wire_id, remote.start)),
            };
            let Some((id, effective_start)) = resolved else { return true; };
            let Some(emote) = resolve(catalog, id) else { return false; };
            rt.generation = rt.generation.wrapping_add(1);
            let generation = rt.generation;
            let client = start.client_num;
            let Some(entity) = snapshot.entities.iter().find(|e| e.number as usize == client) else {
                return false;
            };
            let Some(taunt) = taunt::Taunt::from_animations(
                entity.field_i32("torsoAnim").unwrap_or(-1),
                entity.field_i32("legsAnim").unwrap_or(-1),
            ) else { return false; };
            let flags = entity.field_i32("eFlags").unwrap_or(0);
            if rt.jobs.try_send(Job { client, generation, emote }).is_ok() {
                rt.slots[client] = Some(Remote {
                    generation,
                    wire_id: id,
                    matched: is_match,
                    start: effective_start,
                    flags,
                    taunt,
                    state: PlaybackState::default(),
                });
                progressed = true;
            }
            false
        });
        if !progressed { break; }
    }
}

/// Called before drawing players, including hidden first-person bodies.
pub fn set_render_time(time: i32) {
    runtime().lock().unwrap().render_time = time;
}

pub fn player_preview(entity: u16, viewer_client: i32, time: i32) -> Option<Preview> {
    let mut rt = runtime().lock().unwrap();
    let local_client = rt.own_client.unwrap_or(viewer_client);
    if i32::from(entity) == local_client {
        drop(rt);
        return active_preview();
    }
    let active = rt.slots.get_mut(entity as usize)?.as_mut()?;
    if time < active.start {
        return None;
    }
    preview_at(&mut active.state, time.saturating_sub(active.start))
}

/// The exact remote emote and timeline sample at the current presentation time.
/// The target must already be playing a decoded, visible Fortnite emote.
/// No animation is guessed from a normal JKA/JA+ taunt.
pub fn match_target_preview(client: u16) -> Option<Preview> {
    let mut rt = runtime().lock().ok()?;
    if rt.own_client == Some(i32::from(client)) { return None; }
    let time = rt.render_time;
    let active = rt.slots.get_mut(client as usize)?.as_mut()?;
    if time < active.start || active.state.playing.is_none() { return None; }
    preview_at(&mut active.state, time.saturating_sub(active.start))
}

/// SoundPresenter installs decoded data and drives each player's independent lanes.
pub fn poll_audio() -> (
    Vec<crate::audio::SoundDecodeResult>,
    Vec<(u16, AudioCommand)>,
) {
    let mut rt = runtime().lock().unwrap();
    poll(&mut rt)
}

fn poll(
    rt: &mut Runtime,
) -> (
    Vec<crate::audio::SoundDecodeResult>,
    Vec<(u16, AudioCommand)>,
) {
    let mut audio = Vec::new();
    while let Ok((client, generation, result)) = rt.prepared.try_recv() {
        let Some(active) = rt.slots[client]
            .as_mut()
            .filter(|s| s.generation == generation)
        else {
            continue;
        };
        match result {
            Ok((emote, decoded)) => {
                audio.extend(decoded);
                // A MATCH joins an already-running timeline, so never replay
                // old cues at this player's start. Repeatable cues can fire
                // again on their next loop if their timing supports it.
                let next_audio_ms = if active.matched {
                    audio_times_after(&emote, rt.render_time.saturating_sub(active.start).max(0) as u64)
                } else {
                    emote.audio.iter().map(|c| c.at_ms).collect()
                };
                active.state.playing = Some(Playing {
                    emote,
                    started: Instant::now(),
                    mode: AudioMode::World,
                    next_audio_ms,
                });
                active.state.stop_pending = true;
            }
            Err(error) => {
                log::warn!("remote emote: {error}");
                rt.cancel(client);
            }
        }
    }
    let time = rt.render_time;
    let mut commands = Vec::new();
    for client in 0..MAX_CLIENTS {
        if std::mem::take(&mut rt.stop_pending[client]) {
            commands.push((client as u16, AudioCommand::StopAll));
        }
        if let Some(active) = &mut rt.slots[client] {
            if time >= active.start {
                commands.extend(
                    poll_state_audio(&mut active.state, time.saturating_sub(active.start) as u64)
                        .into_iter()
                        .map(|c| (client as u16, c)),
                );
            }
        }
    }
    (audio, commands)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jka_protocol::{
        gamestate::{ENTITY_FIELDS, EntityState},
        server::PlayerState,
    };

    fn emote(eid: &str, variant: &str) -> Emote {
        jka_emotes::manifest::parse_manifest(&json!({"emotes":[{
            "id":eid,"name":"Test","variant_name":variant,"gla":"test.gla",
            "first_frame":0,"end_frame":300,"fps":30,"loop":true,
            "audio":[{"at_ms":0,"lane":"music","audience":"3p","wav":"test.wav"}]
        }]}))
        .unwrap()
        .remove(0)
    }

    fn dance() -> Emote {
        emote("EID_DanceMoves", "Emote_DanceMoves")
    }

    fn fixture() -> (Runtime, mpsc::Receiver<Job>, mpsc::SyncSender<Prepared>) {
        let (jobs, incoming) = mpsc::sync_channel(MAX_CLIENTS);
        let (completed, prepared) = mpsc::sync_channel(MAX_CLIENTS);
        (
            Runtime {
                own_client: None,
                generation: 0,
                slots: std::array::from_fn(|_| None),
                stop_pending: [false; MAX_CLIENTS],
                render_time: 0,
                jobs,
                prepared,
            },
            incoming,
            completed,
        )
    }

    fn set(es: &mut EntityState, name: &str, bits: u32) {
        es.fields[ENTITY_FIELDS
            .iter()
            .position(|(key, _)| *key == name)
            .unwrap()] = bits;
    }

    fn snapshot(time: i32, clients: &[u16], anim: &str, id: Option<u16>) -> Snapshot {
        let entities = clients
            .iter()
            .map(|&client| {
                let mut es = EntityState {
                    number: client,
                    fields: [0; ENTITY_FIELDS.len()],
                };
                set(&mut es, "eType", 1);
                set(&mut es, "clientNum", u32::from(client));
                set(
                    &mut es,
                    "torsoAnim",
                    jka_assets::animation::animation_index(anim).unwrap() as u32,
                );
                set(
                    &mut es,
                    "legsAnim",
                    jka_assets::animation::animation_index("BOTH_STAND1").unwrap() as u32,
                );
                if let Some(id) = id {
                    let mut angles = [0; 3];
                    assert!(crate::emote_net_codec::encode_cmd_angles(
                        &mut angles,
                        [0; 3],
                        id
                    ));
                    for axis in 0..2 {
                        set(
                            &mut es,
                            &format!("apos.trBase[{axis}]"),
                            ((angles[axis] as i16) as f32 * (360.0 / 65536.0)).to_bits(),
                        );
                    }
                }
                es
            })
            .collect();
        Snapshot {
            server_time: time,
            message_num: time,
            delta_num: 0,
            snap_flags: 0,
            ping: 0,
            server_command_num: 0,
            area_mask: [0; 32],
            player_state: PlayerState::default(),
            vehicle_player_state: None,
            entities,
        }
    }

    #[test]
    fn permanent_ids_survive_reordering_subsets_and_display_changes() {
        assert!(registry().by_id.len() >= 4199);
        let a = dance();
        assert_eq!(wire_id(&a), Some(876)); // permanent protocol assignment
        let b = emote("EID_AbstractMirror", "Emote_AbstractMirror_CMF_M");
        let mut renamed = a.clone();
        renamed.id = "UI group".into();
        renamed.label = "Translated title".into();
        assert_eq!(wire_id(&renamed), wire_id(&a));
        assert_eq!(
            resolve(&[b.clone(), renamed], 876).unwrap().network_key,
            a.network_key
        );
        assert_eq!(
            resolve(&[a.clone(), b.clone()], wire_id(&b).unwrap())
                .unwrap()
                .network_key,
            b.network_key
        );
        assert!(resolve(&[b], 876).is_none()); // missing bundle must never shift the ID
        assert!(resolve(&[a], STOP_ID).is_none());
        assert!(wire_id(&emote("custom_unregistered", "Default")).is_none());
    }

    #[test]
    fn received_taunt_drives_independent_players_at_snapshot_time() {
        let catalog = vec![dance()];
        let id = wire_id(&catalog[0]).unwrap();
        let mut receiver = crate::emote_net::Receiver::default();
        let (mut rt, incoming, completed) = fixture();
        receiver.on_snapshot(&snapshot(100, &[1, 2], "BOTH_STAND1", None), 0);
        let snap = snapshot(150, &[1, 2], "BOTH_ENGAGETAUNT", Some(id));
        let starts = receiver.on_snapshot(&snap, 0);
        assert_eq!(starts.len(), 2);
        update(&mut rt, &snap, 0, &starts, &catalog);
        for _ in 0..2 {
            let job = incoming.try_recv().unwrap();
            completed
                .send((job.client, job.generation, Ok((job.emote, Vec::new()))))
                .unwrap();
        }
        rt.render_time = 125;
        assert!(poll(&mut rt).1.is_empty()); // do not start ahead of interpolation
        rt.render_time = 650;
        let (_, commands) = poll(&mut rt);
        for client in [1, 2] {
            assert!(
                commands
                    .iter()
                    .any(|(c, cmd)| *c == client && matches!(cmd, AudioCommand::Play { .. }))
            );
            let active = rt.slots[client as usize].as_mut().unwrap();
            assert_eq!(preview_at(&mut active.state, 500).unwrap().elapsed_ms, 500);
        }
        // A long dance keeps running after the stock taunt returns to idle.
        update(
            &mut rt,
            &snapshot(3000, &[1, 2], "BOTH_STAND1", None),
            0,
            &[],
            &catalog,
        );
        assert!(rt.slots[1].is_some() && rt.slots[2].is_some());
        rt.cancel(1);
        let (_, commands) = poll(&mut rt);
        assert!(
            commands
                .iter()
                .any(|(c, cmd)| *c == 1 && matches!(cmd, AudioCommand::StopAll))
        );
        assert!(rt.slots[2].as_ref().unwrap().state.playing.is_some());
    }

    fn set_wire(es: &mut EntityState, id: u16) {
        let mut angles = [0; 3];
        assert!(crate::emote_net_codec::encode_cmd_angles(&mut angles, [0; 3], id));
        for axis in 0..2 {
            set(es, &format!("apos.trBase[{axis}]"),
                ((angles[axis] as i16) as f32 * (360.0 / 65536.0)).to_bits());
        }
    }

    fn set_taunt(es: &mut EntityState) {
        set(es, "torsoAnim", jka_assets::animation::animation_index("BOTH_ENGAGETAUNT").unwrap() as u32);
    }

    #[test]
    fn match_propagates_identity_variant_and_origin_and_survives_source_stop() {
        // Client 1 begins the dance. Client 2 matches 1; then 3 matches 2.
        // The third client inherits player 1's *original* timeline, not t=300.
        let catalog = vec![dance()];
        let id = wire_id(&catalog[0]).unwrap();
        let (mut rt, _incoming, _completed) = fixture();
        let mut receiver = crate::emote_net::Receiver::default();
        receiver.on_snapshot(&snapshot(100, &[1, 2, 3], "BOTH_STAND1", None), 0);

        let mut b = snapshot(150, &[1, 2, 3], "BOTH_STAND1", None);
        set_taunt(&mut b.entities[0]);
        set_wire(&mut b.entities[0], id);
        let starts = receiver.on_snapshot(&b, 0);
        assert_eq!(starts.len(), 1);
        update(&mut rt, &b, 0, &starts, &catalog);
        assert!(rt.slots[1].as_ref().unwrap().state.playing.is_none()); // still loading

        let mut a = snapshot(250, &[1, 2, 3], "BOTH_STAND1", None);
        set_taunt(&mut a.entities[0]);
        set_taunt(&mut a.entities[1]);
        set_wire(&mut a.entities[1], crate::emote_net_codec::match_reference_id(1).unwrap());
        let starts = receiver.on_snapshot(&a, 0);
        assert_eq!(starts.len(), 1);
        update(&mut rt, &a, 0, &starts, &catalog);
        let two = rt.slots[2].as_ref().unwrap();
        assert_eq!((two.wire_id, two.start, two.matched), (id, 150, true));

        let mut c = snapshot(300, &[1, 2, 3], "BOTH_STAND1", None);
        for es in c.entities.iter_mut() { set_taunt(es); }
        set_wire(&mut c.entities[2], crate::emote_net_codec::match_reference_id(2).unwrap());
        let starts = receiver.on_snapshot(&c, 0);
        assert_eq!(starts.len(), 1);
        update(&mut rt, &c, 0, &starts, &catalog);
        assert_eq!((rt.slots[3].as_ref().unwrap().wire_id, rt.slots[3].as_ref().unwrap().start), (id, 150));

        let mut stop = c;
        stop.server_time = 350;
        set_wire(&mut stop.entities[0], STOP_ID);
        let starts = receiver.on_snapshot(&stop, 0);
        assert_eq!(starts.len(), 1);
        update(&mut rt, &stop, 0, &starts, &catalog);
        assert!(rt.slots[1].is_none());
        assert_eq!(rt.slots[2].as_ref().unwrap().start, 150);
        assert_eq!(rt.slots[3].as_ref().unwrap().start, 150);
    }

    #[test]
    fn match_source_absent_is_ignored_instead_of_guessing_an_emote() {
        let catalog = vec![dance()];
        let (mut rt, _incoming, _completed) = fixture();
        let snap = snapshot(250, &[2], "BOTH_ENGAGETAUNT", None);
        update(&mut rt, &snap, 0, &[RemoteEmoteStart {
            client_num: 2,
            emote_id: crate::emote_net_codec::match_reference_id(1).unwrap(),
            server_time: 250,
        }], &catalog);
        assert!(rt.slots[2].is_none());
    }

    #[test]
    fn same_snapshot_reference_resolves_even_when_emitter_is_listed_first() {
        let catalog = vec![dance()];
        let id = wire_id(&catalog[0]).unwrap();
        let (mut rt, _incoming, _completed) = fixture();
        let snap = snapshot(250, &[1, 2], "BOTH_ENGAGETAUNT", None);
        let starts = [
            RemoteEmoteStart { client_num: 1,
                emote_id: crate::emote_net_codec::match_reference_id(2).unwrap(), server_time: 250 },
            RemoteEmoteStart { client_num: 2, emote_id: id, server_time: 250 },
        ];
        update(&mut rt, &snap, 0, &starts, &catalog);
        assert_eq!(rt.slots[1].as_ref().unwrap().start, rt.slots[2].as_ref().unwrap().start);
        assert_eq!(rt.slots[1].as_ref().unwrap().wire_id, id);
    }

    #[test]
    fn stop_without_a_new_taunt_and_reset_discard_pending_loads() {
        let catalog = vec![dance()];
        let id = wire_id(&catalog[0]).unwrap();
        let (mut rt, incoming, completed) = fixture();
        let mut receiver = crate::emote_net::Receiver::default();
        receiver.on_snapshot(&snapshot(100, &[1], "BOTH_STAND1", None), 0);
        let snap = snapshot(150, &[1], "BOTH_ENGAGETAUNT", Some(id));
        update(&mut rt, &snap, 0, &receiver.on_snapshot(&snap, 0), &catalog);
        let stale = incoming.try_recv().unwrap();
        let stop = snapshot(200, &[1], "BOTH_ENGAGETAUNT", Some(STOP_ID));
        update(&mut rt, &stop, 0, &receiver.on_snapshot(&stop, 0), &catalog);
        assert!(rt.slots[1].is_none());
        completed
            .send((1, stale.generation, Ok((stale.emote, Vec::new()))))
            .unwrap();
        poll(&mut rt);
        assert!(rt.slots[1].is_none());

        let start = RemoteEmoteStart {
            client_num: 1,
            emote_id: id,
            server_time: 250,
        };
        update(
            &mut rt,
            &snapshot(250, &[1], "BOTH_ENGAGETAUNT", Some(id)),
            0,
            &[start],
            &catalog,
        );
        let stale = incoming.try_recv().unwrap();
        rt.reset();
        update(
            &mut rt,
            &snapshot(300, &[1], "BOTH_ENGAGETAUNT", Some(id)),
            0,
            &[RemoteEmoteStart {
                server_time: 300,
                ..start
            }],
            &catalog,
        );
        completed
            .send((1, stale.generation, Ok((stale.emote, Vec::new()))))
            .unwrap();
        poll(&mut rt);
        assert!(rt.slots[1].as_ref().unwrap().state.playing.is_none());
    }

    #[test]
    fn remote_crouch_obeys_original_taunt_channels_even_after_expiry() {
        let catalog = vec![dance()];
        let id = wire_id(&catalog[0]).unwrap();
        let anim = |name| jka_assets::animation::animation_index(name).unwrap() as u32;
        for (name, full_body) in [("BOTH_ENGAGETAUNT", false), ("BOTH_DUAL_TAUNT", true), ("BOTH_STAFF_TAUNT", true)] {
            for after_expiry in [false, true] {
                let (mut rt, incoming, completed) = fixture();
                let mut snap = snapshot(100, &[1], name, Some(id));
                update(&mut rt, &snap, 0, &[RemoteEmoteStart { client_num: 1, emote_id: id, server_time: 100 }], &catalog);
                let stale = incoming.try_recv().unwrap();
                if full_body {
                    // PM_Weapon takes over the legs once residual velocity settles.
                    set(&mut snap.entities[0], "legsAnim", anim(name));
                    snap.server_time = 150;
                    update(&mut rt, &snap, 0, &[], &catalog);
                }
                if after_expiry {
                    update(&mut rt, &snapshot(3000, &[1], "BOTH_STAND1", None), 0, &[], &catalog);
                    assert!(rt.slots[1].is_some(), "{name} should outlive the native animation");
                }
                let torso = if after_expiry { "BOTH_STAND1" } else { name };
                let mut crouched = snapshot(3100, &[1], torso, None);
                set(&mut crouched.entities[0], "legsAnim", anim("BOTH_CROUCH1"));
                update(&mut rt, &crouched, 0, &[], &catalog);
                assert_eq!(rt.slots[1].is_none(), full_body, "{name}, expired={after_expiry}");
                // Cancellation must discard a load completing after the crouch.
                completed.send((1, stale.generation, Ok((stale.emote, Vec::new())))).unwrap();
                let (_, commands) = poll(&mut rt);
                assert_eq!(rt.slots[1].is_none(), full_body);
                if full_body {
                    assert!(commands.iter().any(|(client, command)| *client == 1 && matches!(command, AudioCommand::StopAll)));
                    assert!(!commands.iter().any(|(_, command)| matches!(command, AudioCommand::Play { .. })));
                } else {
                    assert!(rt.slots[1].as_ref().unwrap().state.playing.is_some());
                }
            }
        }
    }

    #[test]
    fn death_teleport_pvs_exit_and_attack_cancel_remote_playback() {
        let catalog = vec![dance()];
        let id = wire_id(&catalog[0]).unwrap();
        for flags in [1 << 1, 1 << 3, 1 << 9, 1 << 10] {
            let (mut rt, _incoming, _completed) = fixture();
            let snap = snapshot(100, &[1], "BOTH_ENGAGETAUNT", Some(id));
            let start = RemoteEmoteStart {
                client_num: 1,
                emote_id: id,
                server_time: 100,
            };
            update(&mut rt, &snap, 0, &[start], &catalog);
            assert!(rt.slots[1].is_some());
            let mut next = snapshot(150, &[1], "BOTH_ENGAGETAUNT", None);
            set(&mut next.entities[0], "eFlags", flags);
            update(&mut rt, &next, 0, &[], &catalog);
            assert!(rt.slots[1].is_none());
        }
        for (clients, anim) in [
            (vec![], "BOTH_STAND1"),
            (vec![1], "BOTH_JUMP1"),
            (vec![1], "BOTH_CROUCH1"),
            (vec![1], "BOTH_ATTACK1"),
        ] {
            let (mut rt, _incoming, _completed) = fixture();
            let start = RemoteEmoteStart {
                client_num: 1,
                emote_id: id,
                server_time: 100,
            };
            update(
                &mut rt,
                &snapshot(100, &[1], "BOTH_ENGAGETAUNT", Some(id)),
                0,
                &[start],
                &catalog,
            );
            update(
                &mut rt,
                &snapshot(150, &clients, anim, None),
                0,
                &[],
                &catalog,
            );
            assert!(rt.slots[1].is_none(), "{anim}");
        }
    }
}

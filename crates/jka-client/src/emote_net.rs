//! Receive side of DinurdoJK's optional angle-based emote identification.
//! Candidate starts are resolved through the permanent emote registry before playback.

use jka_protocol::server::Snapshot;
use jka_assets::animation::AnimationSet;

use crate::emote_net_codec;

const MAX_CLIENTS: usize = 32;
const SNAPFLAG_SERVERCOUNT: u8 = 1 << 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteEmoteStart {
    pub client_num: usize,
    pub emote_id: u16,
    pub server_time: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AnimState {
    torso: i32,
    legs: i32,
    torso_flip: i32,
    legs_flip: i32,
}

#[derive(Debug, Default)]
pub struct Receiver {
    last: [Option<AnimState>; MAX_CLIENTS],
    last_snapshot_time: Option<i32>,
    last_snapshot_flags: u8,
}

pub(crate) fn is_normal_taunt(anim: i32) -> bool {
    let name = AnimationSet::name(anim & 0x7fff);
    matches!(name,
        Some("BOTH_GESTURE1" | "BOTH_ENGAGETAUNT" | "BOTH_DUAL_TAUNT"
            | "BOTH_STAFF_TAUNT" | "BOTH_ALORA_TAUNT"))
}

impl Receiver {
    pub fn reset(&mut self) { *self = Self::default(); }

    /// Map restarts may advance time while toggling the server-count flag.
    /// Callers also discard queued candidates when this returns true.
    pub fn needs_reset(&self, snapshot: &Snapshot) -> bool {
        self.last_snapshot_time.is_some_and(|old| {
            snapshot.server_time < old
                || ((snapshot.snap_flags ^ self.last_snapshot_flags) & SNAPFLAG_SERVERCOUNT) != 0
        })
    }

    /// Call on newly *decoded* snapshots before visual interpolation.
    /// Never infer an ID from an animation already in progress when a player
    /// first enters PVS, or from entities other than real remote players.
    /// `own_client` is the connection's client number from gamestate, since
    /// playerstate.clientNum belongs to the followed player in spectator mode.
    pub fn on_snapshot(&mut self, snapshot: &Snapshot, own_client: i32) -> Vec<RemoteEmoteStart> {
        if self.needs_reset(snapshot) {
            self.reset();
        }
        if self.last_snapshot_time == Some(snapshot.server_time) { return Vec::new(); }
        self.last_snapshot_time = Some(snapshot.server_time);
        self.last_snapshot_flags = snapshot.snap_flags;
        let mut starts = Vec::new();
        let mut seen = [false; MAX_CLIENTS];
        for es in &snapshot.entities {
            if es.field_i32("eType") != Some(1) { continue; }
            let Some(client_num) = es.field_i32("clientNum").and_then(|v| usize::try_from(v).ok()) else { continue };
            if client_num >= MAX_CLIENTS || client_num as i32 == own_client || usize::from(es.number) != client_num { continue; }
            seen[client_num] = true;
            let Some(torso) = es.field_i32("torsoAnim") else { continue };
            let Some(legs) = es.field_i32("legsAnim") else { continue };
            let state = AnimState {
                torso,
                legs,
                torso_flip: es.field_i32("torsoFlip").unwrap_or(0),
                legs_flip: es.field_i32("legsFlip").unwrap_or(0),
            };
            let previous = self.last[client_num].replace(state);
            // Only a newly started taunt, not a held taunt, may carry an ID.
            let Some(old) = previous else { continue };
            // Explicit cosmetic stop has a reserved complete codeword and
            // needs no new game taunt (the server may still be holding one).
            let decoded = es.field_f32("apos.trBase[1]").zip(es.field_f32("apos.trBase[0]"))
                .and_then(|(yaw, pitch)| emote_net_codec::decode_snapshot_angles(yaw, pitch));
            if decoded == Some(crate::emotes::network::STOP_ID) {
                starts.push(RemoteEmoteStart { client_num, emote_id: crate::emotes::network::STOP_ID, server_time: snapshot.server_time });
                continue;
            }
            let torso_started = is_normal_taunt(torso)
                && (old.torso != torso || old.torso_flip != state.torso_flip);
            let legs_started = is_normal_taunt(legs)
                && (old.legs != legs || old.legs_flip != state.legs_flip);
            if !(torso_started || legs_started) { continue; }
            if let Some(emote_id) = decoded {
                starts.push(RemoteEmoteStart { client_num, emote_id, server_time: snapshot.server_time });
            }
        }
        // A client returning to PVS must acquire a fresh baseline, not compare
        // its taunt to an arbitrarily old animation state.
        for (slot, found) in seen.iter().enumerate() {
            if !found { self.last[slot] = None; }
        }
        starts
    }
}

#[cfg(test)]
#[path = "emote_net_tests.rs"]
mod tests;

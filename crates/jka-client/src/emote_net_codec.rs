//! Single-snapshot, client-only emote identifier codec for JKA protocol 26.
//!
//! This uses *existing* 16-bit usercmd view angles and server-replicated
//! `entityState_t.apos.trBase`. No wire changes, server patches, or userinfo.
//! Version 2 uses 13 ID bits and 9 signature bits in the same 22 angle bits.
//! Its magic is only a probabilistic check, NOT peer authentication.

pub const EMOTE_ID_BITS: u32 = 13;
pub const ANGLE_PAYLOAD_BITS: u32 = 11;
pub const EMOTE_CAPACITY: u16 = 1 << EMOTE_ID_BITS;
// Reserved v2 IDs. 8191 remains STOP. 8128..8159 reference a player
// instead of a catalog emote, retaining the existing aim-angle transport.
pub const MATCH_BASE_ID: u16 = 8128;
pub const MATCH_CLIENT_COUNT: u16 = 32;

pub fn match_reference_id(client: u16) -> Option<u16> {
    (client < MATCH_CLIENT_COUNT).then(|| MATCH_BASE_ID + client)
}

pub fn match_source_client(id: u16) -> Option<u16> {
    (MATCH_BASE_ID..MATCH_BASE_ID + MATCH_CLIENT_COUNT)
        .contains(&id)
        .then(|| id - MATCH_BASE_ID)
}
pub const SIGNATURE: u16 = 0x16d; // version 2; immutable once deployed
const MASK: u16 = (1 << ANGLE_PAYLOAD_BITS) - 1;
const PERIOD: i32 = 1 << ANGLE_PAYLOAD_BITS;

/// Preserve the nearest original angle while choosing an exact low-bit codeword.
/// The worst-case signed difference is 1024 short steps = 5.625 degrees.
pub fn nearest_angle_short(original: i32, desired_low: u16) -> i32 {
    let current = original & i32::from(MASK);
    let wanted = i32::from(desired_low & MASK);
    let delta = (wanted - current + PERIOD / 2).rem_euclid(PERIOD) - PERIOD / 2;
    original.wrapping_add(delta) & 0xffff
}

pub fn payload(id: u16) -> Option<u32> {
    (id < EMOTE_CAPACITY).then_some((u32::from(SIGNATURE) << EMOTE_ID_BITS) | u32::from(id))
}

/// `delta_angles` is the server-side additive correction to usercmd angles.
/// The raw outgoing *command* gets the inverse correction so that the server's
/// subsequent viewangles, not merely the command values, contain the payload.
/// Returns None when pitch is too near a clamp to preserve the encoded bits.
pub fn encode_cmd_angles(cmd: &mut [i32; 3], delta_angles: [i32; 3], id: u16) -> bool {
    let Some(bits) = payload(id) else { return false };
    let yaw = 1usize;
    let pitch = 0usize;
    let encoded_yaw = nearest_angle_short(cmd[yaw].wrapping_add(delta_angles[yaw]), (bits & u32::from(MASK)) as u16);
    let encoded_pitch = nearest_angle_short(cmd[pitch].wrapping_add(delta_angles[pitch]), (bits >> ANGLE_PAYLOAD_BITS) as u16);
    // OpenJK PM_UpdateViewAngles clamps the pitch to approximately +/-89
    // degrees. If that clamp would change this packet, do not claim success.
    let pitch_degrees = short_to_signed_degrees(encoded_pitch);
    if !(-88.0..=88.0).contains(&pitch_degrees) {
        return false;
    }
    cmd[yaw] = encoded_yaw.wrapping_sub(delta_angles[yaw]) & 0xffff;
    cmd[pitch] = encoded_pitch.wrapping_sub(delta_angles[pitch]) & 0xffff;
    true
}

fn short_to_signed_degrees(short: i32) -> f64 {
    let signed = (short as i16) as f64;
    signed * (360.0 / 65536.0)
}

/// Decode the un-interpolated raw snapshot degrees (not client-render angles).
/// Rounds to the nearest JKA 16-bit short to tolerate float32 serialization.
fn degrees_to_short(degrees: f32) -> Option<u16> {
    if !degrees.is_finite() { return None; }
    let steps = ((degrees as f64) * (65536.0 / 360.0)).round() as i64;
    Some((steps & 65535) as u16)
}

pub fn decode_snapshot_angles(yaw_degrees: f32, pitch_degrees: f32) -> Option<u16> {
    let yaw = u32::from(degrees_to_short(yaw_degrees)? & MASK);
    let pitch = u32::from(degrees_to_short(pitch_degrees)? & MASK);
    let packet = yaw | (pitch << ANGLE_PAYLOAD_BITS);
    if (packet >> EMOTE_ID_BITS) as u16 != SIGNATURE { return None; }
    Some((packet & u32::from(EMOTE_CAPACITY - 1)) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_refs_round_trip_without_overlapping_stop() {
        for client in 0..MATCH_CLIENT_COUNT {
            let id = match_reference_id(client).unwrap();
            assert_eq!(match_source_client(id), Some(client));
            let mut cmd = [2048, 16384, 0];
            assert!(encode_cmd_angles(&mut cmd, [0; 3], id));
            let pitch = short_to_signed_degrees(cmd[0]) as f32;
            let yaw = (cmd[1] as f64 * (360.0 / 65536.0)) as f32;
            assert_eq!(match_source_client(decode_snapshot_angles(yaw, pitch).unwrap()), Some(client));
        }
        assert_eq!(match_reference_id(32), None);
        assert_eq!(match_source_client(8191), None); // STOP
        assert_eq!(match_source_client(4200), None); // ordinary catalog ID
    }

    #[test]
    fn all_emotes_and_aim_quadrants_round_trip() {
        for id in 0..EMOTE_CAPACITY {
            for &(pitch, yaw) in &[(0, 0), (8192, 16384), (61000, 65000), (1300, 30000)] {
                let mut cmd = [pitch, yaw, 0];
                assert!(encode_cmd_angles(&mut cmd, [0; 3], id));
                let pitch_float = short_to_signed_degrees(cmd[0]) as f32;
                let yaw_float = (cmd[1] as f64 * (360.0 / 65536.0)) as f32;
                assert_eq!(decode_snapshot_angles(yaw_float, pitch_float), Some(id));
            }
        }
    }

    #[test]
    fn delta_angles_are_compensated() {
        let mut cmd = [1700, 21900, 0];
        let delta = [124, 28000, 0];
        assert!(encode_cmd_angles(&mut cmd, delta, 1799));
        let yaw = ((cmd[1] + delta[1]) & 65535) as f32 * (360.0 / 65536.0);
        let pitch = short_to_signed_degrees((cmd[0] + delta[0]) & 65535) as f32;
        assert_eq!(decode_snapshot_angles(yaw, pitch), Some(1799));
    }

    #[test]
    fn invalid_ids_and_clamped_pitch_are_safe() {
        assert_eq!(payload(EMOTE_CAPACITY), None);
        let mut cmd = [-16000, 0, 0];
        assert!(!encode_cmd_angles(&mut cmd, [0; 3], 23));
    }

    #[test]
    fn max_adjustment_is_bounded() {
        for original in (0..65536).step_by(53) {
            let encoded = nearest_angle_short(original, 1799);
            let delta = ((encoded - original + 32768) & 65535) - 32768;
            assert!(delta.abs() <= 1024);
        }
    }
}

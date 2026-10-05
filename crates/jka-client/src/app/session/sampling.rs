//! Session sampling.
use crate::app::{
    scene, snapshot_discontinuity, DemoCameraSample, GameSession, PlayerViewPolicyState,
};

impl GameSession {
    pub(in crate::app) fn sample_at(
        &self,
        presentation_time_ms: f64,
        server_time: i32,
        teleported: bool,
    ) -> Option<DemoCameraSample> {
        let current = self.current_snapshot.as_ref()?;
        let next = self.next_snapshot.as_ref();
        // Match CG_SetNextSnap's demo interpolation guards: abrupt teleports,
        // follow-target changes, and map_restart server-count toggles must not
        // blend the recorded camera through space.
        let interpolation_discontinuity =
            next.is_some_and(|next| snapshot_discontinuity(current, next));
        let alpha = if interpolation_discontinuity {
            0.0
        } else {
            next.filter(|next| next.server_time > current.server_time)
                .map(|next| {
                    ((presentation_time_ms - f64::from(current.server_time))
                        / f64::from(next.server_time - current.server_time))
                        as f32
                })
                .unwrap_or(0.0)
                .clamp(0.0, 1.0)
        };

        let current_origin = playerstate_vec3(&current.player_state, "origin")?;
        let current_angles = playerstate_vec3(&current.player_state, "viewangles")?;
        let current_velocity =
            playerstate_vec3(&current.player_state, "velocity").unwrap_or([0.0; 3]);
        let current_viewheight = current.player_state.field_i32("viewheight").unwrap_or(0) as f32;

        let (origin, angles, velocity, viewheight) = if let Some(next) = next {
            let next_origin =
                playerstate_vec3(&next.player_state, "origin").unwrap_or(current_origin);
            let next_angles =
                playerstate_vec3(&next.player_state, "viewangles").unwrap_or(current_angles);
            let next_velocity =
                playerstate_vec3(&next.player_state, "velocity").unwrap_or(current_velocity);
            let next_viewheight = next
                .player_state
                .field_i32("viewheight")
                .unwrap_or(current_viewheight as i32) as f32;
            (
                lerp_vec3(current_origin, next_origin, alpha),
                [
                    lerp_angle_degrees(current_angles[0], next_angles[0], alpha),
                    lerp_angle_degrees(current_angles[1], next_angles[1], alpha),
                    lerp_angle_degrees(current_angles[2], next_angles[2], alpha),
                ],
                lerp_vec3(current_velocity, next_velocity, alpha),
                current_viewheight + (next_viewheight - current_viewheight) * alpha,
            )
        } else {
            (
                current_origin,
                current_angles,
                current_velocity,
                current_viewheight,
            )
        };

        let policy = PlayerViewPolicyState::from_player_state(&current.player_state);
        let mut eye = origin;
        if policy.pm_type != crate::cgame::PM_INTERMISSION {
            eye[2] += viewheight;
        }
        Some(DemoCameraSample {
            server_time,
            presentation_time_ms,
            native_origin: origin,
            eye_position: scene::render_position(eye),
            player_position: scene::render_position(origin),
            view_angles: angles,
            velocity,
            bob_cycle: current.player_state.field_i32("bobCycle").unwrap_or(0),
            view_height: viewheight.round() as i32,
            dead_yaw: current.player_state.stats.get(6).copied().unwrap_or(0) as f32,
            client_num: current.player_state.field_i32("clientNum").unwrap_or(0),
            policy,
            following: current.player_state.field_i32("pm_flags").unwrap_or(0) & 4096 != 0,
            teleported,
        })
    }
}

/// Camera sample for cg.predictedPlayerState (CG_CalcViewValues adds the
/// decaying prediction error to the view origin only).
pub(in crate::app) fn sample_player_state(
    ps: &jka_protocol::server::PlayerState,
    server_time: i32,
    teleported: bool,
    error: [f32; 3],
) -> Option<DemoCameraSample> {
    let origin = playerstate_vec3(ps, "origin")?;
    let view_angles = playerstate_vec3(ps, "viewangles")?;
    let velocity = playerstate_vec3(ps, "velocity").unwrap_or([0.0; 3]);
    let view_height = ps.field_i32("viewheight").unwrap_or(0);
    let policy = PlayerViewPolicyState::from_player_state(ps);
    let view_origin = [
        origin[0] + error[0],
        origin[1] + error[1],
        origin[2] + error[2],
    ];
    let mut eye = view_origin;
    if policy.pm_type != crate::cgame::PM_INTERMISSION {
        eye[2] += view_height as f32;
    }
    Some(DemoCameraSample {
        server_time,
        presentation_time_ms: f64::from(server_time),
        native_origin: view_origin,
        eye_position: scene::render_position(eye),
        player_position: scene::render_position(origin),
        view_angles,
        velocity,
        bob_cycle: ps.field_i32("bobCycle").unwrap_or(0),
        view_height,
        dead_yaw: ps.stats.get(6).copied().unwrap_or(0) as f32,
        client_num: ps.field_i32("clientNum").unwrap_or(0),
        policy,
        following: ps.field_i32("pm_flags").unwrap_or(0) & 4096 != 0,
        teleported,
    })
}

pub(in crate::app) fn playerstate_vec3(
    state: &jka_protocol::server::PlayerState,
    base: &str,
) -> Option<[f32; 3]> {
    Some([
        state.field_f32(&format!("{base}[0]"))?,
        state.field_f32(&format!("{base}[1]"))?,
        state.field_f32(&format!("{base}[2]"))?,
    ])
}

/// TaystJK `CG_IsWalkingAnim` for spectator/demo strafehelper command
/// reconstruction. Resolve the animation ids once from the shared JKA table so
/// this stays correct without duplicating enum numbers in the client.
pub(in crate::app) fn strafehelper_walking_anim(raw_anim: i32) -> bool {
    const ANIM_TOGGLEBIT: i32 = 2048;
    static WALK_ANIMS: std::sync::OnceLock<[i32; 8]> = std::sync::OnceLock::new();
    let walk_anims = WALK_ANIMS.get_or_init(|| {
        [
            "BOTH_WALK1",
            "BOTH_WALK2",
            "BOTH_WALK_STAFF",
            "BOTH_WALKBACK_STAFF",
            "BOTH_WALK_DUAL",
            "BOTH_WALKBACK_DUAL",
            "BOTH_WALKBACK1",
            "BOTH_WALKBACK2",
        ]
        .map(|name| {
            jka_assets::animation::animation_index(name)
                .and_then(|index| i32::try_from(index).ok())
                .unwrap_or(-1)
        })
    });
    walk_anims.contains(&(raw_anim & !ANIM_TOGGLEBIT))
}

pub(in crate::app) fn lerp_vec3(a: [f32; 3], b: [f32; 3], alpha: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * alpha,
        a[1] + (b[1] - a[1]) * alpha,
        a[2] + (b[2] - a[2]) * alpha,
    ]
}

pub(in crate::app) fn lerp_angle_degrees(a: f32, b: f32, alpha: f32) -> f32 {
    let delta = (b - a + 180.0).rem_euclid(360.0) - 180.0;
    a + delta * alpha
}

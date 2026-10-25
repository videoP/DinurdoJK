//! Session sampling.
use crate::app::{scene, DemoCameraSample, GameSession, PlayerViewPolicyState};

/// Input sampled for this presentation frame. Renderer late input remains a
/// camera effect after this state has been published.
#[derive(Clone, Copy)]
pub(in crate::app) enum PlayerPresentationInput {
    Local([f32; 3]),
    Remote { angles: [f32; 3], no_predict: bool },
}

impl GameSession {
    /// Rust counterpart of cg.predictedPlayerState for read-only consumers.
    pub(in crate::app) fn presented_player_state(
        &self,
    ) -> Option<&jka_protocol::server::PlayerState> {
        self.presented_player_state.as_ref()
    }

    pub(in crate::app) fn update_presented_player_state(
        &mut self,
        presentation_time_ms: f64,
        server_time: i32,
        teleported: bool,
        prediction: Option<&crate::app::LivePredictionFrame>,
        input: Option<PlayerPresentationInput>,
    ) -> Option<DemoCameraSample> {
        let mut ps = present_player_state(
            self.current_snapshot.as_ref(),
            self.next_snapshot.as_ref(),
            presentation_time_ms,
            prediction,
            input,
        )?;
        // Prediction failures fall back to snapshots, which can themselves
        // contain NaNs. Keep those out of the camera, models and audio too.
        let fallback = self.current_snapshot.as_ref()
            .map(|snapshot| &snapshot.player_state)
            .filter(|state| finite_playerstate_vec3(state, "origin").is_some())
            .or(self.presented_player_state.as_ref());
        repair_presentation_coordinates(&mut ps, fallback);
        let error = prediction.map_or([0.0; 3], |prediction| prediction.error);
        let mut sample = sample_player_state(&ps, server_time, teleported, error)?;
        sample.presentation_time_ms = presentation_time_ms;
        self.presented_player_state = Some(ps);
        Some(sample)
    }
}

fn finite_playerstate_vec3(ps: &jka_protocol::server::PlayerState, field: &str) -> Option<[f32; 3]> {
    playerstate_vec3(ps, field).filter(|values| values.iter().all(|v| v.is_finite()))
}

/// Only repair the render copy. Preserve the decoder's exact snapshot/delta
/// history and leave authority with the server until a valid state arrives.
fn repair_presentation_coordinates(
    ps: &mut jka_protocol::server::PlayerState,
    previous: Option<&jka_protocol::server::PlayerState>,
) {
    let previous = previous.filter(|previous| previous.field_i32("clientNum") == ps.field_i32("clientNum"));
    if finite_playerstate_vec3(ps, "origin").is_none() {
        // A reconnect can start with an already-invalid player. Keep a finite
        // render-only placeholder then, so the connection/console stays usable
        // for /kill instead of failing camera setup before a respawn can arrive.
        let origin = previous.and_then(|previous| finite_playerstate_vec3(previous, "origin"))
            .unwrap_or([0.0; 3]);
        for (axis, value) in origin.into_iter().enumerate() {
            ps.set_field_bits(&format!("origin[{axis}]"), value.to_bits());
        }
    }
    for field in ["velocity", "viewangles"] {
        if finite_playerstate_vec3(ps, field).is_none() {
            let values = if field == "viewangles" {
                previous.and_then(|previous| finite_playerstate_vec3(previous, field)).unwrap_or([0.0; 3])
            } else {
                [0.0; 3]
            };
            for (axis, value) in values.into_iter().enumerate() {
                ps.set_field_bits(&format!("{field}[{axis}]"), value.to_bits());
            }
        }
    }
}

/// Select prediction's provisional display or the interpolated snapshot, then
/// apply this frame's input ownership before any presentation consumer reads it.
fn present_player_state(
    current: Option<&jka_protocol::server::Snapshot>,
    next: Option<&jka_protocol::server::Snapshot>,
    presentation_time_ms: f64,
    prediction: Option<&crate::app::LivePredictionFrame>,
    input: Option<PlayerPresentationInput>,
) -> Option<jka_protocol::server::PlayerState> {
    let mut ps = match prediction {
        Some(prediction) => prediction.display.clone(),
        None => crate::cgame::player_state::interpolate_player_state_at(
            current?,
            next,
            presentation_time_ms,
        ),
    };
    apply_presentation_input(
        &mut ps,
        prediction,
        input,
        current.map(|snapshot| &snapshot.player_state),
    );
    Some(ps)
}

fn apply_presentation_input(
    ps: &mut jka_protocol::server::PlayerState,
    prediction: Option<&crate::app::LivePredictionFrame>,
    input: Option<PlayerPresentationInput>,
    snapshot: Option<&jka_protocol::server::PlayerState>,
) {
    // Follow/demo angles belong to the player being viewed, never our input.
    if ps.field_i32("pm_flags").unwrap_or(0) & 4096 != 0 {
        return;
    }
    let angles = match input {
        Some(PlayerPresentationInput::Local(angles)) => Some(angles),
        Some(PlayerPresentationInput::Remote { angles, no_predict }) => {
            if ps.field_i32("pm_type").unwrap_or(0) >= 5 {
                return;
            }
            if let Some(prediction) = prediction {
                if prediction.view_forced {
                    return;
                }
                prediction.command_angles.and_then(|command| {
                    let base = playerstate_vec3(ps, "viewangles")?;
                    let delta = |axis: usize| {
                        let now = jka_movement::angle_to_short(angles[axis]);
                        (now.wrapping_sub(command[axis]) as i16) as f32 * (360.0 / 65536.0)
                    };
                    Some([
                        (base[0] + delta(0))
                            .clamp(-16000.0 * 360.0 / 65536.0, 16000.0 * 360.0 / 65536.0),
                        base[1] + delta(1),
                        base[2],
                    ])
                })
            } else if no_predict {
                // CG_InterpolatePlayerState(grabAngles): PM_UpdateViewAngles.
                let angle = |axis: usize| {
                    let command = jka_movement::angle_to_short(angles[axis]);
                    let delta = ps.field_i32(&format!("delta_angles[{axis}]")).unwrap_or(0);
                    i32::from(command.wrapping_add(delta) as i16)
                };
                Some([
                    angle(0).clamp(-16000, 16000) as f32 * (360.0 / 65536.0),
                    angle(1) as f32 * (360.0 / 65536.0),
                    // The existing no-predict input path preserves snapshot
                    // roll; never take it from the interpolated next angles.
                    snapshot
                        .unwrap_or(ps)
                        .field_f32("viewangles[2]")
                        .unwrap_or(0.0),
                ])
            } else {
                None
            }
        }
        None => None,
    };
    if let Some(angles) = angles {
        for (axis, angle) in angles.into_iter().enumerate() {
            ps.set_field_bits(&format!("viewangles[{axis}]"), angle.to_bits());
        }
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
    let origin = finite_playerstate_vec3(ps, "origin")?;
    let view_angles = finite_playerstate_vec3(ps, "viewangles")?;
    let velocity = finite_playerstate_vec3(ps, "velocity").unwrap_or([0.0; 3]);
    let error = if error.iter().all(|v| v.is_finite()) { error } else { [0.0; 3] };
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
        pm_flags: ps.field_i32("pm_flags").unwrap_or(0),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::LivePredictionFrame;
    use jka_protocol::server::PlayerState;

    #[test]
    fn non_finite_snapshot_presentation_holds_last_position_and_recovers_on_respawn() {
        let mut previous = PlayerState::default();
        previous.set_field_bits("origin[0]", (-60.261955f32).to_bits());
        previous.set_field_bits("origin[1]", (-2554.4631f32).to_bits());
        previous.set_field_bits("origin[2]", (-1934.9353f32).to_bits());
        let mut bad = previous.clone();
        bad.set_field_bits("origin[0]", f32::NAN.to_bits());
        bad.set_field_bits("origin[2]", (-2147483600.0f32).to_bits());
        bad.set_field_bits("velocity[0]", f32::NAN.to_bits());
        let wire = bad.clone();
        assert!(sample_player_state(&bad, 10, false, [0.0; 3]).is_none());
        repair_presentation_coordinates(&mut bad, Some(&previous));
        let sample = sample_player_state(&bad, 10, false, [f32::NAN; 3]).unwrap();
        assert_eq!(sample.native_origin, playerstate_vec3(&previous, "origin").unwrap());
        assert_eq!(sample.velocity, [0.0; 3]);
        assert!(wire.field_f32("origin[0]").unwrap().is_nan());
        // A valid respawn is used immediately, without retaining the old hold.
        let mut respawn = PlayerState::default();
        respawn.set_field_bits("origin[0]", 500.0f32.to_bits());
        let expected = respawn.clone();
        repair_presentation_coordinates(&mut respawn, Some(&bad));
        assert_eq!(respawn, expected);
        assert_eq!(sample_player_state(&respawn, 20, true, [0.0; 3]).unwrap().native_origin, [500.0, 0.0, 0.0]);
    }

    #[test]
    fn non_finite_presentation_does_not_borrow_another_players_position() {
        let mut previous = PlayerState::default();
        previous.set_field_bits("origin[0]", 123.0f32.to_bits());
        let mut bad = previous.clone();
        bad.set_field_bits("clientNum", 2);
        bad.set_field_bits("origin[1]", f32::INFINITY.to_bits());
        repair_presentation_coordinates(&mut bad, Some(&previous));
        assert_eq!(sample_player_state(&bad, 10, false, [0.0; 3]).unwrap().native_origin, [0.0; 3]);
    }

    fn prediction() -> LivePredictionFrame {
        let mut display = PlayerState::default();
        display.set_field_bits("origin[0]", 123.5f32.to_bits());
        display.set_field_bits("velocity[0]", 333.25f32.to_bits());
        display.set_field_bits("viewangles[1]", 15.0f32.to_bits());
        display.set_field_bits("commandTime", 108);
        LivePredictionFrame {
            committed: PlayerState::default(),
            previous_committed: PlayerState::default(),
            display,
            error: [1.0, 2.0, 3.0],
            command_angles: Some([0, jka_movement::angle_to_short(15.0), 0]),
            view_forced: false,
        }
    }

    #[test]
    fn provisional_movement_and_new_mouse_input_survive_presentation() {
        let prediction = prediction();
        let original = prediction.display.clone();
        let ps = present_player_state(
            None,
            None,
            108.0,
            Some(&prediction),
            Some(PlayerPresentationInput::Remote {
                angles: [0.0, 30.0, 0.0],
                no_predict: false,
            }),
        )
        .unwrap();
        assert_eq!(ps.field_i32("commandTime"), Some(108));
        assert_eq!(ps.field_f32("origin[0]"), Some(123.5));
        assert_eq!(ps.field_f32("velocity[0]"), Some(333.25));
        assert!(ps.field_f32("viewangles[1]").unwrap() > 29.99);
        assert_eq!(prediction.display, original); // never commit display-only view changes
        let sample = sample_player_state(&ps, 108, false, prediction.error).unwrap();
        assert_eq!(
            sample.view_angles,
            playerstate_vec3(&ps, "viewangles").unwrap()
        );
        assert_eq!(sample.velocity, playerstate_vec3(&ps, "velocity").unwrap());
        assert_eq!(
            scene::jka_position(sample.player_position),
            [123.5, 0.0, 0.0]
        );
        assert_eq!(sample.native_origin, [124.5, 2.0, 3.0]); // error offsets camera only
        let entity = crate::cgame::presented_player_state_entity(&ps).unwrap();
        assert_eq!(entity.origin, [123.5, 0.0, 0.0]);
        assert_eq!(entity.angles, sample.view_angles);
    }

    #[test]
    fn short_angle_wrap_and_pitch_limits_preserve_immediate_input() {
        let mut prediction = prediction();
        prediction.command_angles = Some([0, jka_movement::angle_to_short(359.0), 0]);
        prediction
            .display
            .set_field_bits("viewangles[1]", 359.0f32.to_bits());
        let mut ps = prediction.display.clone();
        apply_presentation_input(
            &mut ps,
            Some(&prediction),
            Some(PlayerPresentationInput::Remote {
                angles: [100.0, 1.0, 0.0],
                no_predict: false,
            }),
            None,
        );
        assert!(ps.field_f32("viewangles[1]").unwrap() > 360.99);
        assert_eq!(
            ps.field_f32("viewangles[0]"),
            Some(16000.0 * 360.0 / 65536.0)
        );
    }

    #[test]
    fn no_predict_grabs_input_and_current_snapshot_delta_angles() {
        let mut ps = PlayerState::default();
        ps.set_field_bits("delta_angles[1]", 16384);
        apply_presentation_input(
            &mut ps,
            None,
            Some(PlayerPresentationInput::Remote {
                angles: [0.0, 45.0, 0.0],
                no_predict: true,
            }),
            None,
        );
        assert_eq!(ps.field_f32("viewangles[1]"), Some(135.0));
    }

    #[test]
    fn follow_dead_and_forced_views_never_take_spectator_mouse_input() {
        let mut prediction = prediction();
        let input = Some(PlayerPresentationInput::Remote {
            angles: [60.0, 170.0, 0.0],
            no_predict: true,
        });
        prediction.display.set_field_bits("pm_flags", 4096);
        let mut ps = prediction.display.clone();
        apply_presentation_input(&mut ps, Some(&prediction), input, None);
        assert_eq!(ps, prediction.display);
        apply_presentation_input(&mut ps, None, input, None);
        assert_eq!(ps, prediction.display);
        apply_presentation_input(
            &mut ps,
            None,
            Some(PlayerPresentationInput::Local([60.0, 170.0, 0.0])),
            None,
        );
        assert_eq!(ps, prediction.display);
        prediction.display.set_field_bits("pm_flags", 0);
        prediction.display.set_field_bits("pm_type", 5);
        ps = prediction.display.clone();
        apply_presentation_input(&mut ps, Some(&prediction), input, None);
        assert_eq!(ps, prediction.display);
        prediction.display.set_field_bits("pm_type", 0);
        prediction.view_forced = true;
        ps = prediction.display.clone();
        apply_presentation_input(&mut ps, Some(&prediction), input, None);
        assert_eq!(ps, prediction.display);
    }

    #[test]
    fn local_subframe_angles_are_published_without_quantization() {
        let mut ps = PlayerState::default();
        let angles = [12.34567, 45.67891, 0.0];
        apply_presentation_input(
            &mut ps,
            None,
            Some(PlayerPresentationInput::Local(angles)),
            None,
        );
        assert_eq!(playerstate_vec3(&ps, "viewangles"), Some(angles));
        let sample = sample_player_state(&ps, 100, false, [0.0; 3]).unwrap();
        assert_eq!(sample.view_angles, angles);
    }

    #[test]
    fn follow_camera_and_body_share_state_but_eye_uses_current_viewheight() {
        let mut current = PlayerState::default();
        current.set_field_bits("pm_flags", 4096);
        current.set_field_bits("viewheight", 26);
        current.set_field_bits("bobCycle", 250);
        let mut next = current.clone();
        next.set_field_bits("origin[0]", 100.0f32.to_bits());
        next.set_field_bits("velocity[0]", 400.0f32.to_bits());
        next.set_field_bits("viewangles[1]", 180.0f32.to_bits());
        next.set_field_bits("viewheight", 12);
        next.set_field_bits("bobCycle", 5);
        let ps = crate::cgame::player_state::interpolate_player_state(&current, Some(&next), 0.75);
        let sample = sample_player_state(&ps, 100, false, [0.0; 3]).unwrap();
        let entity = crate::cgame::presented_player_state_entity(&ps).unwrap();
        assert_eq!(entity.origin, sample.native_origin);
        assert_eq!(entity.angles, sample.view_angles);
        assert_eq!(sample.velocity, [300.0, 0.0, 0.0]);
        assert_eq!(sample.bob_cycle, 258);
        assert_eq!(sample.view_height, 26);
        let eye = scene::jka_position(sample.eye_position);
        assert_eq!(eye, [75.0, 0.0, 26.0]);
    }
    #[test]
    fn no_predict_roll_stays_on_current_snapshot() {
        let mut current = PlayerState::default();
        current.set_field_bits("viewangles[2]", 10.0f32.to_bits());
        let mut next = current.clone();
        next.set_field_bits("viewangles[2]", 30.0f32.to_bits());
        let mut ps =
            crate::cgame::player_state::interpolate_player_state(&current, Some(&next), 0.5);
        apply_presentation_input(
            &mut ps,
            None,
            Some(PlayerPresentationInput::Remote {
                angles: [0.0, 45.0, 0.0],
                no_predict: true,
            }),
            Some(&current),
        );
        assert_eq!(ps.field_f32("viewangles[2]"), Some(10.0));
    }
}

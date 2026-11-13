//! Session sampling.
use crate::app::{scene, DemoCameraSample, GameSession, PlayerViewPolicyState};

/// Input sampled for this presentation frame. Renderer late input remains a
/// camera effect after this state has been published.
#[derive(Clone, Copy)]
pub(in crate::app) enum PlayerPresentationInput {
    Local([f32; 3]),
    Remote { angles: [f32; 3], no_predict: bool },
}

// TaystJK cg_view.c::CG_CalcFov owns `zoomFov` on the CGame side. The
// playerState only networks zoomMode/zoomLocked (and an approximate hold
// duration in zoomFov); it does NOT network the per-frame optical FOV.
// Keep that presentation value apart from native Pmove and reuse it for
// both the scene projection and CG_DrawZoomMask artwork.
#[derive(Debug, Clone, Copy, Default)]
pub(in crate::app) struct ScopeZoomState {
    last: Option<ScopeZoomSample>,
}

#[derive(Debug, Clone, Copy)]
struct ScopeZoomSample {
    client_num: i32,
    spawn_count: i32,
    mode: i32,
    fov: f32,
    time_ms: f64,
}

impl ScopeZoomState {
    pub(in crate::app) fn fov(self) -> Option<f32> {
        self.last.filter(|last| last.mode == 1 || last.mode == 2)
            .map(|last| last.fov)
    }

    pub(in crate::app) fn update(&mut self, ps: &jka_protocol::server::PlayerState,
        time_ms: f64, reset: bool)
    {
        const MAX_ZOOM_FOV: f32 = 3.0; // TaystJK cg_local.h
        let client_num = ps.field_i32("clientNum").unwrap_or(0);
        let spawn_count = ps.persistant[4]; // PERS_SPAWN_COUNT
        let mode = ps.field_i32("zoomMode").unwrap_or(0);
        let locked = ps.field_i32("zoomLocked").unwrap_or(0) != 0;
        let previous = self.last.filter(|last| !reset && time_ms.is_finite()
            && time_ms >= last.time_ms && time_ms - last.time_ms <= 250.0
            && client_num == last.client_num && spawn_count == last.spawn_count
            && mode == last.mode);
        let delta_ms = previous.map_or(0.0, |last| (time_ms - last.time_ms) as f32);
        let fov = match mode {
            1 => {
                if let Some(last) = previous {
                    if locked { last.fov } else { (last.fov.min(50.0) - delta_ms * 0.035).max(MAX_ZOOM_FOV) }
                } else if locked {
                    // Demo join/seek can start with an already locked scope.
                    // Pmove's *networked* zoomFov is the held-time progress,
                    // not the actual optical FOV (50 - progress).
                    let progress = ps.field_f32("zoomFov").unwrap_or(0.0);
                    if progress.is_finite() { (50.0 - progress).clamp(MAX_ZOOM_FOV, 50.0) }
                    else { 50.0 }
                } else { 50.0 }
            }
            2 => previous.map_or(80.0, |last| (last.fov - delta_ms * 0.075).max(40.0)),
            _ => 80.0, // TaystJK resets zoomFov outside the weapon/binocular mode.
        };
        self.last = Some(ScopeZoomSample { client_num, spawn_count, mode, fov, time_ms });
    }
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
        // PMF_FOLLOW is a server-owned camera, not a predicted local player.
        // The remote entity smoother has already sampled this frame's visual
        // time. Apply that SAME clock to the followed player's snapshot PS.
        // Only origin, angles and velocity of this render copy are touched;
        // event, HUD, prediction and protocol state keep their original clock.
        if self.live && crate::cgame::remote_smoothing::mode() != 0 {
            self.remote_smoothing.apply_follow(&mut ps, std::time::Instant::now(), teleported);
        }
        // The ordinary snapshot playerState is interpolated above when nextSnap
        // exists. With no nextSnap, that interpolation holds the last origin
        // while the live camera's damping clock continues to advance. Keep
        // the missing-snapshot fix strictly in spectator follow, and only in
        // legacy remote-smoothing mode: modes 1-4 already resample the followed
        // player with their own visual clock/correction history.
        if self.live && crate::cgame::remote_smoothing::mode() == 0 {
            self.spectator_follow_hold.apply(
                &mut ps,
                self.current_snapshot.as_ref(),
                self.next_snapshot.as_ref(),
                presentation_time_ms,
                teleported,
            );
        } else {
            self.spectator_follow_hold.reset();
        }
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
        // TaystJK CG_TransitionPlayerState / CG_OffsetFirstPersonView:
        // viewheight changes instantly in Pmove, but the first-person eye
        // eases over DUCK_TIME (100 ms). Never modify the predicted PS or
        // the discrete view_height used by third-person camera collision.
        self.duck_view_height.apply(
            &ps,
            &mut sample,
            presentation_time_ms,
            teleported || self.seeking_demo,
        );
        self.scope_zoom.update(&ps, presentation_time_ms, teleported || self.seeking_demo);
        self.presented_player_state = Some(ps);
        Some(sample)
    }
}

// Short, live-spectator-only extension of a held snapshot playerState.
// Normal local prediction, recorded demos, and the third-person damping
// functions must never use this state. Follow and camera geometry both consume
// the same adjusted *presentation copy* of the playerState.
//
// These limits match the existing live remote-smoothing visual strategy:
// extrapolate at most 35 ms; correct small extrapolation errors over 65 ms;
// discard a correction larger than 32 world units instead of rubber-banding.
const SPECTATOR_FOLLOW_EXTRAP_MS: f64 = 35.0;
const SPECTATOR_FOLLOW_CORRECTION_MS: f32 = 65.0;
const SPECTATOR_FOLLOW_MAX_CORRECTION: f32 = 32.0;

#[derive(Clone, Copy)]
struct SpectatorFollowFrame {
    client_num: i32,
    spawn_count: i32,
    pm_type: i32,
    vehicle_num: i32,
    snapshot_time: i32,
    sampled_at_ms: f64,
    presented_origin: [f32; 3],
    velocity: [f32; 3],
    correction: [f32; 3],
    extrapolated: bool,
}

#[derive(Default)]
pub(in crate::app) struct SpectatorFollowHold {
    last: Option<SpectatorFollowFrame>,
}

impl SpectatorFollowHold {
    pub(in crate::app) fn reset(&mut self) { self.last = None; }

    fn apply(
        &mut self,
        ps: &mut jka_protocol::server::PlayerState,
        current: Option<&jka_protocol::server::Snapshot>,
        next: Option<&jka_protocol::server::Snapshot>,
        time_ms: f64,
        teleported: bool,
    ) {
        const PMF_FOLLOW: i32 = 4096;
        let Some(current) = current else { self.reset(); return; };
        if ps.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW == 0
            || !time_ms.is_finite()
        {
            self.reset();
            return;
        }
        let Some(raw) = finite_playerstate_vec3(ps, "origin") else { self.reset(); return; };
        let Some(velocity) = finite_playerstate_vec3(ps, "velocity") else { self.reset(); return; };
        let client_num = ps.field_i32("clientNum").unwrap_or(-1);
        let spawn_count = ps.persistant[4];
        let pm_type = ps.field_i32("pm_type").unwrap_or(-1);
        let vehicle_num = ps.field_i32("m_iVehicleNum").unwrap_or(0);
        let previous = self.last.filter(|last| {
            !teleported
                && last.client_num == client_num
                && last.spawn_count == spawn_count
                && last.pm_type == pm_type
                && last.vehicle_num == vehicle_num
                && last.snapshot_time <= current.server_time
                && time_ms >= last.sampled_at_ms
                && time_ms - last.sampled_at_ms <= 200.0
        });
        let dt_ms = previous.map_or(0.0, |last| (time_ms - last.sampled_at_ms) as f32);
        // A future snapshot means the ordinary playerState was interpolated.
        // Never replace those coordinates with a parallel approximation.
        // Also don't extend dead/vehicle camera states or a changed follow POV.
        let held_ms = if previous.is_some() && next.is_none()
            && vehicle_num == 0 && ps.stats[0] > 0
        {
            (time_ms - f64::from(current.server_time))
                .clamp(0.0, SPECTATOR_FOLLOW_EXTRAP_MS)
        } else { 0.0 };
        let mut origin = raw;
        if held_ms > 0.0 {
            let extrapolated: [f32; 3] = std::array::from_fn(|axis| {
                raw[axis] + velocity[axis] * (held_ms as f32 * 0.001)
            });
            let delta: [f32; 3] = std::array::from_fn(|axis| extrapolated[axis] - raw[axis]);
            let length = delta.iter().map(|value| value * value).sum::<f32>().sqrt();
            let scale = if length > SPECTATOR_FOLLOW_MAX_CORRECTION {
                SPECTATOR_FOLLOW_MAX_CORRECTION / length
            } else { 1.0 };
            for axis in 0..3 { origin[axis] += delta[axis] * scale; }
        }
        let mut correction = [0.0; 3];
        if let Some(last) = previous {
            correction = std::array::from_fn(|axis| {
                last.correction[axis] * (-dt_ms / SPECTATOR_FOLLOW_CORRECTION_MS).exp()
            });
            if last.extrapolated && current.server_time > last.snapshot_time {
                // Reconcile a held/extrapolated pose with the newly arrived
                // authoritative snapshot. The visual correction is discarded
                // at teleports, follow switches, and large position jumps.
                let elapsed = (dt_ms * 0.001).min(0.05);
                correction = std::array::from_fn(|axis| {
                    last.presented_origin[axis] + last.velocity[axis] * elapsed - origin[axis]
                });
            }
            if correction.iter().map(|value| value * value).sum::<f32>()
                > SPECTATOR_FOLLOW_MAX_CORRECTION.powi(2)
            {
                correction = [0.0; 3];
            }
            for axis in 0..3 { origin[axis] += correction[axis]; }
        }
        self.last = Some(SpectatorFollowFrame {
            client_num, spawn_count, pm_type, vehicle_num,
            snapshot_time: current.server_time,
            sampled_at_ms: time_ms,
            presented_origin: origin,
            velocity,
            correction,
            extrapolated: held_ms > 0.0,
        });
        if held_ms > 0.0 || correction != [0.0; 3] {
            for (axis, value) in origin.into_iter().enumerate() {
                ps.set_field_bits(&format!("origin[{axis}]"), value.to_bits());
            }
        }
    }
}

// TaystJK cg_playerstate.c CG_TransitionPlayerState records changes to
// playerState.viewheight; cg_view.c CG_OffsetFirstPersonView interpolates
// those changes for DUCK_TIME. Keep this separate from network/player state
// and the third-person viewheight, so Pmove and collision stay authoritative.
const DUCK_TIME_MS: f64 = 100.0;

#[derive(Clone, Copy, Debug)]
struct TrackedDuckHeight {
    client_num: i32,
    following: bool,
    pm_type: i32,
    team: i32,
    spawn_count: i32,
    target: i32,
    from: f32,
    changed_at_ms: f64,
    sampled_at_ms: f64,
}

impl TrackedDuckHeight {
    fn height_at(self, time_ms: f64) -> f32 {
        let fraction = ((time_ms - self.changed_at_ms) / DUCK_TIME_MS).clamp(0.0, 1.0) as f32;
        self.from + (self.target as f32 - self.from) * fraction
    }
}

#[derive(Default)]
pub(in crate::app) struct DuckViewHeight {
    last: Option<TrackedDuckHeight>,
}

impl DuckViewHeight {
    fn height(&mut self, ps: &jka_protocol::server::PlayerState, time_ms: f64, reset: bool) -> f32 {
        const PMF_FOLLOW: i32 = 4096;
        let height = ps.field_i32("viewheight").unwrap_or(0);
        let client_num = ps.field_i32("clientNum").unwrap_or(0);
        let following = ps.field_i32("pm_flags").unwrap_or(0) & PMF_FOLLOW != 0;
        let pm_type = ps.field_i32("pm_type").unwrap_or(0);
        let team = ps.persistant[3]; // PERS_TEAM
        let spawn_count = ps.persistant[4]; // PERS_SPAWN_COUNT
        let last = self.last;
        // Never blend two POVs, a respawn, a teleport, or a demo seek.
        // The large time-gap check also protects resumed/suspended sessions.
        let discontinuity = reset || !time_ms.is_finite() || last.is_none_or(|last| {
            client_num != last.client_num
                || following != last.following
                || pm_type != last.pm_type
                || team != last.team
                || spawn_count != last.spawn_count
                || time_ms < last.sampled_at_ms
                || time_ms - last.sampled_at_ms > 250.0
        });
        if discontinuity {
            self.last = Some(TrackedDuckHeight {
                client_num, following, pm_type, team, spawn_count,
                target: height, from: height as f32,
                changed_at_ms: time_ms, sampled_at_ms: time_ms,
            });
            return height as f32;
        }
        let mut last = last.expect("duck height continuity checked");
        if last.target != height {
            // A quick crouch tap can reverse an unfinished transition. Start
            // from the *visible* eye height, not the old target, to avoid pops.
            last.from = last.height_at(time_ms);
            last.target = height;
            last.changed_at_ms = time_ms;
        }
        last.sampled_at_ms = time_ms;
        let presented = last.height_at(time_ms);
        self.last = Some(last);
        presented
    }

    fn apply(
        &mut self,
        ps: &jka_protocol::server::PlayerState,
        sample: &mut DemoCameraSample,
        presentation_time_ms: f64,
        reset: bool,
    ) {
        let displayed_height = self.height(ps, presentation_time_ms, reset);
        if sample.policy.pm_type != crate::cgame::PM_INTERMISSION {
            let mut eye = sample.native_origin;
            eye[2] += displayed_height;
            sample.eye_position = scene::render_position(eye);
        }
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
    state.field_vec3(base)
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

    fn live_follow_snapshot(time: i32, client: i32, spawn: i32, origin: f32, velocity: f32)
        -> jka_protocol::server::Snapshot
    {
        use jka_protocol::server::Snapshot;
        let mut ps = PlayerState::default();
        ps.set_field_bits("pm_flags", 4096); // PMF_FOLLOW
        ps.set_field_bits("clientNum", client as u32);
        ps.persistant[4] = spawn;
        ps.stats[0] = 100;
        ps.set_field_bits("origin[0]", origin.to_bits());
        ps.set_field_bits("velocity[0]", velocity.to_bits());
        Snapshot {
            server_time: time,
            message_num: time,
            delta_num: -1,
            snap_flags: 0,
            ping: 0,
            server_command_num: 0,
            area_mask: [0; 32],
            player_state: ps,
            vehicle_player_state: None,
            entities: vec![],
        }
    }

    #[test]
    fn spectator_follow_uses_existing_interpolated_origin_when_next_snapshot_exists() {
        let a = live_follow_snapshot(100, 2, 1, 0.0, 400.0);
        let b = live_follow_snapshot(150, 2, 1, 100.0, 400.0);
        let mut ps = present_player_state(Some(&a), Some(&b), 125.0, None, None).unwrap();
        assert_eq!(ps.field_f32("origin[0]"), Some(50.0));
        let mut hold = SpectatorFollowHold::default();
        hold.apply(&mut ps, Some(&a), Some(&b), 125.0, false);
        assert_eq!(ps.field_f32("origin[0]"), Some(50.0));
        // The snapshot-interpolated PS is also the followed-player body origin.
        let sample = sample_player_state(&ps, 125, false, [0.0; 3]).unwrap();
        let body = crate::cgame::presented_player_state_entity(&ps).unwrap();
        assert_eq!(body.origin, sample.native_origin);
    }

    #[test]
    fn spectator_follow_only_advances_missing_snapshot_for_35ms() {
        let current = live_follow_snapshot(100, 2, 1, 0.0, 400.0);
        let mut hold = SpectatorFollowHold::default();
        let mut ps = current.player_state.clone();
        hold.apply(&mut ps, Some(&current), None, 100.0, false);
        assert_eq!(ps.field_f32("origin[0]"), Some(0.0));
        let mut ps = current.player_state.clone();
        hold.apply(&mut ps, Some(&current), None, 120.0, false);
        assert_eq!(ps.field_f32("origin[0]"), Some(8.0));
        let mut ps = current.player_state.clone();
        hold.apply(&mut ps, Some(&current), None, 170.0, false);
        assert_eq!(ps.field_f32("origin[0]"), Some(14.0)); // capped
    }

    #[test]
    fn spectator_follow_corrects_new_snapshot_without_changing_network_state() {
        let a = live_follow_snapshot(100, 2, 1, 0.0, 400.0);
        let b = live_follow_snapshot(150, 2, 1, 18.0, 400.0);
        let mut hold = SpectatorFollowHold::default();
        let mut ps = a.player_state.clone();
        hold.apply(&mut ps, Some(&a), None, 100.0, false);
        let mut ps = a.player_state.clone();
        hold.apply(&mut ps, Some(&a), None, 130.0, false);
        assert_eq!(ps.field_f32("origin[0]"), Some(12.0));
        let mut presented = b.player_state.clone();
        hold.apply(&mut presented, Some(&b), None, 150.0, false);
        // The camera doesn't snap instantly from the extrapolated 12 to 18.
        assert!((presented.field_f32("origin[0]").unwrap() - 20.0).abs() < 0.001);
        assert_eq!(b.player_state.field_f32("origin[0]"), Some(18.0));
        let mut presented = b.player_state.clone();
        hold.apply(&mut presented, Some(&b), None, 158.0, false);
        let corrected = presented.field_f32("origin[0]").unwrap();
        assert!(corrected > 22.0 && corrected < 23.2);
    }

    #[test]
    fn spectator_follow_never_extrapolates_other_views_or_across_follow_switches() {
        let a = live_follow_snapshot(100, 2, 1, 0.0, 400.0);
        let b = live_follow_snapshot(150, 3, 1, 100.0, 400.0);
        let mut hold = SpectatorFollowHold::default();
        let mut ps = a.player_state.clone();
        hold.apply(&mut ps, Some(&a), None, 100.0, false);
        let mut ps = a.player_state.clone();
        hold.apply(&mut ps, Some(&a), None, 120.0, false);
        assert_eq!(ps.field_f32("origin[0]"), Some(8.0));
        let mut switched = b.player_state.clone();
        hold.apply(&mut switched, Some(&b), None, 180.0, false);
        assert_eq!(switched.field_f32("origin[0]"), Some(100.0));
        switched.set_field_bits("pm_flags", 0);
        hold.apply(&mut switched, Some(&b), None, 190.0, false);
        assert_eq!(switched.field_f32("origin[0]"), Some(100.0));
        let mut teleported = a.player_state.clone();
        hold.apply(&mut teleported, Some(&a), None, 200.0, true);
        assert_eq!(teleported.field_f32("origin[0]"), Some(0.0));
    }

    #[test]
    fn disruptor_optical_zoom_animates_and_locks_without_using_networked_fov() {
        let mut ps = PlayerState::default();
        let mut zoom = ScopeZoomState::default();
        ps.set_field_bits("zoomMode", 1);
        ps.set_field_bits("zoomFov", 80.0f32.to_bits());
        zoom.update(&ps, 1000.0, false);
        assert_eq!(zoom.fov(), Some(50.0));
        zoom.update(&ps, 1200.0, false);
        assert!((zoom.fov().unwrap() - 43.0).abs() < 0.001);
        ps.set_field_bits("zoomLocked", 1);
        ps.set_field_bits("zoomFov", 7.0f32.to_bits());
        zoom.update(&ps, 1250.0, false);
        assert!((zoom.fov().unwrap() - 43.0).abs() < 0.001);
        zoom.update(&ps, 1350.0, false);
        assert!((zoom.fov().unwrap() - 43.0).abs() < 0.001);
        ps.set_field_bits("zoomMode", 0);
        zoom.update(&ps, 1400.0, false);
        assert!(zoom.fov().is_none());
        ps.set_field_bits("zoomMode", 1);
        ps.set_field_bits("zoomLocked", 0);
        zoom.update(&ps, 1500.0, false);
        assert_eq!(zoom.fov(), Some(50.0));
    }

    #[test]
    fn zoom_initializes_locked_demo_and_resets_on_seek_or_pov_change() {
        let mut ps = PlayerState::default();
        ps.set_field_bits("zoomMode", 1);
        ps.set_field_bits("zoomLocked", 1);
        ps.set_field_bits("zoomFov", 31.0f32.to_bits());
        let mut zoom = ScopeZoomState::default();
        zoom.update(&ps, 5000.0, false);
        assert_eq!(zoom.fov(), Some(19.0));
        ps.set_field_bits("zoomLocked", 0);
        zoom.update(&ps, 5020.0, false);
        assert!((zoom.fov().unwrap() - 18.3).abs() < 0.001);
        ps.set_field_bits("clientNum", 2);
        zoom.update(&ps, 5040.0, false);
        assert_eq!(zoom.fov(), Some(50.0));
        ps.set_field_bits("zoomLocked", 1);
        zoom.update(&ps, 1000.0, true);
        assert_eq!(zoom.fov(), Some(19.0));
    }

    #[test]
    fn binocular_zoom_progresses_independently_of_weapon_scope() {
        let mut ps = PlayerState::default();
        ps.set_field_bits("zoomMode", 2);
        let mut zoom = ScopeZoomState::default();
        zoom.update(&ps, 1000.0, false);
        assert_eq!(zoom.fov(), Some(80.0));
        zoom.update(&ps, 1200.0, false);
        assert_eq!(zoom.fov(), Some(65.0));
        zoom.update(&ps, 1400.0, false);
        assert_eq!(zoom.fov(), Some(50.0));
        zoom.update(&ps, 1500.0, false);
        assert_eq!(zoom.fov(), Some(42.5));
        zoom.update(&ps, 1550.0, false);
        assert_eq!(zoom.fov(), Some(40.0));
    }

    #[test]
    fn spectator_crouch_and_stand_ease_camera_over_tayst_duck_time_only() {
        let mut ps = PlayerState::default();
        ps.set_field_bits("pm_type", jka_movement::PM_SPECTATOR as u32);
        ps.set_field_bits("viewheight", 26);
        let mut smoothing = DuckViewHeight::default();
        let mut eye_at = |time: f64, height: i32| {
            ps.set_field_bits("viewheight", height as u32);
            let mut sample = sample_player_state(&ps, time as i32, false, [0.0; 3]).unwrap();
            smoothing.apply(&ps, &mut sample, time, false);
            assert_eq!(sample.view_height, height); // raw PS/third-person is untouched
            assert_eq!(ps.field_i32("viewheight"), Some(height));
            scene::jka_position(sample.eye_position)[2]
        };
        assert_eq!(eye_at(1000.0, 26), 26.0);
        assert_eq!(eye_at(1010.0, 12), 26.0); // no pop at the crouch edge
        assert_eq!(eye_at(1060.0, 12), 19.0);
        assert_eq!(eye_at(1110.0, 12), 12.0);
        assert_eq!(eye_at(1120.0, 26), 12.0); // stand up also eases
        assert_eq!(eye_at(1170.0, 26), 19.0);
        assert_eq!(eye_at(1220.0, 26), 26.0);
    }

    #[test]
    fn interrupted_crouch_reverses_from_visible_height_without_jump() {
        let mut ps = PlayerState::default();
        ps.set_field_bits("viewheight", 26);
        let mut smoothing = DuckViewHeight::default();
        assert_eq!(smoothing.height(&ps, 1000.0, false), 26.0);
        ps.set_field_bits("viewheight", 12);
        assert_eq!(smoothing.height(&ps, 1010.0, false), 26.0);
        assert_eq!(smoothing.height(&ps, 1060.0, false), 19.0);
        ps.set_field_bits("viewheight", 26);
        assert_eq!(smoothing.height(&ps, 1060.0, false), 19.0);
        assert_eq!(smoothing.height(&ps, 1110.0, false), 22.5);
        assert_eq!(smoothing.height(&ps, 1160.0, false), 26.0);
    }

    #[test]
    fn duck_smoothing_resets_on_follow_switch_respawn_teleport_and_seek() {
        let mut ps = PlayerState::default();
        ps.set_field_bits("viewheight", 26);
        let mut smoothing = DuckViewHeight::default();
        smoothing.height(&ps, 1000.0, false);
        ps.set_field_bits("viewheight", 12);
        assert_eq!(smoothing.height(&ps, 1010.0, false), 26.0);
        ps.set_field_bits("clientNum", 3);
        assert_eq!(smoothing.height(&ps, 1020.0, false), 12.0);
        ps.set_field_bits("viewheight", 26);
        assert_eq!(smoothing.height(&ps, 1030.0, false), 12.0);
        ps.set_field_bits("pm_flags", 4096);
        assert_eq!(smoothing.height(&ps, 1040.0, false), 26.0);
        ps.set_field_bits("viewheight", 12);
        assert_eq!(smoothing.height(&ps, 1050.0, false), 26.0);
        ps.persistant[4] += 1; // PERS_SPAWN_COUNT
        assert_eq!(smoothing.height(&ps, 1060.0, false), 12.0);
        ps.set_field_bits("viewheight", 26);
        smoothing.height(&ps, 1070.0, false);
        assert_eq!(smoothing.height(&ps, 1080.0, true), 26.0); // teleport
        ps.set_field_bits("viewheight", 12);
        smoothing.height(&ps, 1090.0, false);
        assert_eq!(smoothing.height(&ps, 800.0, false), 12.0); // rewind
        ps.set_field_bits("viewheight", 26);
        assert_eq!(smoothing.height(&ps, 810.0, true), 26.0); // seek
    }

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
            hidden_entities: Vec::new(),
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

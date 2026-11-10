//! TaystJK CG_Draw2D / CG_OffsetThirdPersonView falling-to-death presentation.
//! The server owns `fallingToDeath` (milliseconds). This module only holds the
//! camera anchor, draws the timed fade, and optionally requests early respawn.
use super::{App, DemoCameraSample};

/// TaystJK `FALL_FADE_TIME / 2`: 3000 / 2 milliseconds.
const FALL_FADE_MS: f64 = 1_500.0;
const AUTO_ATTACK_INTERVAL_MS: f64 = 500.0;

#[derive(Clone, Copy, Debug)]
pub(super) struct FallingDeathCamera {
    pub(super) anchor: [f32; 3], // native JKA xyz, not render-space xyz
    pub(super) start: i32,
    pub(super) last_presented: f64,
    pub(super) kill_requested: bool,
    pub(super) last_auto_attack: Option<f64>,
}

impl FallingDeathCamera {
    fn alpha(self) -> f32 {
        ((self.last_presented - f64::from(self.start)) / FALL_FADE_MS).clamp(0.0, 1.0) as f32
    }
}

impl App {
    /// OpenJK/TaystJK uses the first falling snapshot as `gCGFallVector` and
    /// keeps it fixed until `fallingToDeath` clears. Never infer death from Z.
    pub(super) fn update_falling_death(&mut self, sample: &DemoCameraSample) {
        let start = sample.policy.falling_to_death;
        let is_player = sample.policy.team != 3
            && sample.policy.pm_type != jka_movement::PM_SPECTATOR
            && sample.policy.pm_type != crate::cgame::PM_INTERMISSION
            && !sample.following && !self.live_input.is_jk2();
        if start == 0 || !is_player {
            self.reset_falling_death();
            return;
        }
        let time = sample.presentation_time_ms;
        if self.falling_death.is_none_or(|fall| {
            fall.start != start || sample.teleported || time < fall.last_presented
        }) {
            self.falling_death = Some(FallingDeathCamera {
                anchor: sample.native_origin,
                start,
                last_presented: time,
                kill_requested: false,
                last_auto_attack: None,
            });
        } else if let Some(fall) = self.falling_death.as_mut() {
            fall.last_presented = time;
        }

        // TaystJK CG_Draw2D sends a `kill` request and an attack pulse with
        // `wait 2` while falling. Request kill once per fall; repeat the attack
        // pulse at a controlled interval instead of filling the command buffer
        // every 1000+ FPS render frame. A unique input owner prevents the
        // auto-release from releasing a key the player is physically holding.
        // Never send gameplay commands in demo playback or while following.
        let may_kill = self.japro_cg.auto_kill_when_falling
            && self.game_session.as_ref().is_some_and(|session| session.live && !session.local)
            && self.live_connected();
        if may_kill && self.falling_death.is_some_and(|fall| !fall.kill_requested) {
            if self.queue_server_command("kill").is_ok() {
                if let Some(fall) = self.falling_death.as_mut() {
                    fall.kill_requested = true;
                }
            }
        }
        if may_kill && self.falling_death.is_some_and(|fall| {
            fall.last_auto_attack.is_none_or(|last| time - last >= AUTO_ATTACK_INTERVAL_MS)
        }) {
            self.append_console_commands(super::split_console_script(
                "+attack @bind:autofall; wait 2; -attack @bind:autofall",
            ));
            if let Some(fall) = self.falling_death.as_mut() {
                fall.last_auto_attack = Some(time);
            }
        }
    }

    pub(super) fn reset_falling_death(&mut self) {
        if self.falling_death.take().is_some() {
            self.execute_input_button_command("-attack @bind:autofall");
        }
    }

    /// Apply TaystJK's post-CG_CalcViewValues anchor/viewaxis override. The
    /// look vector is formed from the ordinary third-person eye to the player,
    /// *before* replacing the eye with the saved fall origin.
    pub(super) fn apply_falling_death_camera(&mut self, sample: &DemoCameraSample) {
        let Some(fall) = self.falling_death else { return };
        let player = glam::Vec3::from_array(crate::scene::render_position(sample.native_origin));
        let look = player - self.camera.position;
        if look.length_squared() > 1.0e-6 {
            self.camera.yaw = (-look.z).atan2(look.x);
            self.camera.pitch = look.y.atan2(look.x.hypot(look.z));
        }
        self.camera.position = glam::Vec3::from_array(crate::scene::render_position(fall.anchor));
        self.camera.clear_first_person_bob();
    }

    pub(super) fn falling_death_alpha(&self) -> f32 {
        self.falling_death.map_or(0.0, FallingDeathCamera::alpha)
    }

    pub(super) fn falling_death_alpha_byte(&self) -> u8 {
        (self.falling_death_alpha() * 255.0).round() as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_uses_server_timestamp_and_clamps() {
        let state = FallingDeathCamera {
            anchor: [0.0; 3], start: 2000, last_presented: 2000.0,
            kill_requested: false, last_auto_attack: None,
        };
        assert_eq!(state.alpha(), 0.0);
        assert_eq!(FallingDeathCamera { last_presented: 2750.0, ..state }.alpha(), 0.5);
        assert_eq!(FallingDeathCamera { last_presented: 3600.0, ..state }.alpha(), 1.0);
        assert_eq!(FallingDeathCamera { last_presented: 1900.0, ..state }.alpha(), 0.0);
    }
}

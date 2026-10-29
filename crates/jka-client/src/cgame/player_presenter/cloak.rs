//! TaystJK CG_Player's `cent->cloaked` / `cent->uncloaking` presentation state.
//! The wire format only carries PW_CLOAKED, so a local per-entity clock owns
//! the two-second visual crossfade. This uses game/demo time, not wall time.

const CLOAK_FADE_MS: i32 = 2_000;

use super::{Arc, DynamicModelSurface, PlayerPresenter};

#[derive(Clone, Copy, Debug)]
pub(super) struct ForceRefractionState {
    pub start: i32,
    pub sampled_at: i32,
    pub origin: [f32; 3],
}

impl ForceRefractionState {
    pub(super) fn advance(prior: Option<Self>, now: i32, origin: [f32; 3], pull: bool) -> (Self, f32, f32) {
        let mut state = prior.filter(|s| now >= s.sampled_at)
            .unwrap_or(Self { start: now, sampled_at: now, origin });
        let elapsed = now.saturating_sub(state.start);
        if elapsed < 200 { state.origin = origin; }
        state.sampled_at = now;
        let remaining = (500 - elapsed).max(0) as f32;
        let scale = (if pull { elapsed as f32 } else { remaining } * 0.003).clamp(0.2, 1.0);
        let alpha = (remaining * 0.488).clamp(10.0, 244.0) / 255.0;
        (state, scale, alpha)
    }
}

impl PlayerPresenter {
    /// Custom shaders replace all authored skin stages, once per mesh.
    pub(super) fn cloak_shells(&mut self, body: &[DynamicModelSurface], strength: f32, axis: [[f32; 3]; 3], origin: [f32; 3]) -> Vec<DynamicModelSurface> {
        use jka_assets::shader::{Shader, TcMod};
        let shader = "gfx/effects/cloakedShader";
        let seconds = self.stage_time_ms as f32 * 0.001;
        let (texture, mode, glow) = self.custom_shader_material(shader);
        let uv = self.custom_shader_uv_xform(shader, seconds);
        let env = self.custom_shader_env_map(shader);
        let effective = self.shader_remaps.effective_name(shader);
        let turbulence = jka_assets::shader::find_shader(&self.shaders, effective.as_ref())
            .and_then(Shader::primary)
            .and_then(|stage| stage.tc_mods.iter().find_map(|m| match m {
                TcMod::Turb { amplitude, phase, frequency, .. } => Some([*amplitude, *phase + seconds * *frequency]),
                _ => None,
            })).unwrap_or([0.0; 2]);
        let mut seen = std::collections::HashSet::new();
        let mut shells = Vec::new();
        for base in body {
            let key = base.ghoul2_gpu.as_ref().map_or_else(
                || format!("cpu:{:p}", Arc::as_ptr(&base.indices)),
                |gpu| gpu.mesh_key.to_string());
            if !seen.insert(key) { continue; }
            let mut shell = Self::clone_surface_with_material_xform(
                base, texture.clone(), mode, [strength; 4], Some(uv), Some(0.0), Some(env),
            );
            if let Some(gpu) = &mut shell.ghoul2_gpu {
                // The two spare UV coefficients carry RB_CalcTurbulentTexCoords.
                gpu.uv_cross[2] = turbulence[0];
                gpu.uv_cross[3] = turbulence[1];
                gpu.specular = None;
            } else {
                let eye = self.stage_view_position.unwrap_or([0.0; 3]);
                shell.vertices = Arc::new(base.vertices.iter().map(|vertex| {
                    let mut v = *vertex;
                    let st = if env { super::environment_uv(v.position, v.normal, eye) } else { v.uv };
                    v.uv = [st[0] * uv[0] + st[1] * uv[2] + uv[4], st[0] * uv[1] + st[1] * uv[3] + uv[5]];
                    // CPU vertices are in render space; turbulence uses model space.
                    let delta = [v.position[0] - origin[0], -v.position[2] - origin[1], v.position[1] - origin[2]];
                    let local = axis.map(|a| {
                        let norm = a.iter().map(|x| x*x).sum::<f32>().max(0.0001);
                        (0..3).map(|i| a[i]*delta[i]).sum::<f32>() / norm
                    });
                    let phase = [(local[0] + local[2]) / 1024.0 + turbulence[1],
                        local[1] / 1024.0 + turbulence[1]];
                    for i in 0..2 { v.uv[i] += (phase[i] * std::f32::consts::TAU).sin() * turbulence[0]; }
                    v.color = [strength; 4];
                    v
                }).collect());
            }
            shell.lighting_origin = None;
            shell.authored_glow = glow;
            shell.rt_rigid = None;
            shell.rt_skinned_key = None;
            shells.push(shell);
        }
        shells
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum CloakVisual {
    Normal,
    /// Cloak-shell strength and normal-skin alpha, each 0..1.
    Transition { shell: f32, skin: f32 },
    Cloaked,
    /// Source ektogamat/predator-material: procedural physical cloak.
    Predator { progress: f32 },
}

#[derive(Clone, Copy, Debug)]
pub(super) struct CloakState {
    cloaked: bool,
    until: i32,
    sampled_at: i32,
    predator_progress: f32,
    predator_velocity: f32,
}

impl CloakState {
    /// Direct port of maath/easing damp's critically-damped scalar update,
    /// called in the reference with smoothTime=1.2 and the frame delta.
    /// Unlike the TaystJK 2000ms fade this retains velocity through an
    /// interrupted transition and runs on game/demo time.
    pub(super) fn advance_predator(prior: Option<Self>, cloaked: bool, now: i32) -> (Self, CloakVisual) {
        let prior = prior.filter(|old| now >= old.sampled_at);
        let mut state = prior.unwrap_or(Self {
            cloaked,
            until: now,
            sampled_at: now,
            predator_progress: 0.0,
            predator_velocity: 0.0,
        });
        let dt = ((now - state.sampled_at).max(0) as f32 * 0.001).min(0.25);
        let target = if cloaked { 1.0 } else { 0.0 };
        if (state.predator_progress - target).abs() < 0.001 {
            state.predator_progress = target;
            state.predator_velocity = 0.0;
        } else if dt > 0.0 {
            let omega = 2.0 / 1.2;
            let x = omega * dt;
            let decay = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x);
            let change = state.predator_progress - target;
            let temp = (state.predator_velocity + omega * change) * dt;
            state.predator_velocity = (state.predator_velocity - omega * temp) * decay;
            state.predator_progress = (target + (change + temp) * decay).clamp(0.0, 1.0);
        }
        state.cloaked = cloaked;
        state.sampled_at = now;
        let visual = if cloaked || state.predator_progress > 0.001 {
            CloakVisual::Predator { progress: state.predator_progress }
        } else {
            CloakVisual::Normal
        };
        (state, visual)
    }

    pub(super) fn advance(prior: Option<Self>, cloaked: bool, now: i32) -> (Self, CloakVisual) {
        // A time rewind happens when switching/reseeking a demo. Never carry
        // an expiry in the *future* from the previous playback position.
        let prior = prior.filter(|prior| now >= prior.sampled_at);
        let state = match prior {
            Some(old) if old.cloaked != cloaked => Self {
                cloaked,
                until: now.saturating_add(CLOAK_FADE_MS),
                sampled_at: now,
                predator_progress: old.predator_progress,
                predator_velocity: old.predator_velocity,
            },
            Some(old) => Self { sampled_at: now, ..old },
            None => Self {
                cloaked,
                until: if cloaked { now.saturating_add(CLOAK_FADE_MS) } else { now },
                sampled_at: now,
                predator_progress: 0.0,
                predator_velocity: 0.0,
            },
        };
        let visual = if state.until > now {
            let remaining = (state.until - now) as f32 / CLOAK_FADE_MS as f32;
            // TaystJK: perc = (uncloaking - cg.time)/2000; if cloaking,
            // reverse it. The skin layer gets alpha = 1 - perc.
            let shell = if cloaked { 1.0 - remaining } else { remaining };
            CloakVisual::Transition { shell, skin: 1.0 - shell }
        } else if cloaked {
            CloakVisual::Cloaked
        } else {
            CloakVisual::Normal
        };
        (state, visual)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_refraction_matches_taystjk_push_pull_and_hand_follow_window() {
        let (start, scale, alpha) = ForceRefractionState::advance(None, 1000, [1.0; 3], false);
        assert_eq!(scale, 1.0);
        assert!((alpha - 244.0 / 255.0).abs() < 1e-6);
        let (follow, _, _) = ForceRefractionState::advance(Some(start), 1199, [2.0; 3], false);
        assert_eq!(follow.origin, [2.0; 3]);
        let (fixed, scale, _) = ForceRefractionState::advance(Some(follow), 1400, [3.0; 3], false);
        assert_eq!(fixed.origin, [2.0; 3]);
        assert!((scale - 0.3).abs() < 1e-6);
        let (_, scale, alpha) = ForceRefractionState::advance(Some(fixed), 1600, [4.0; 3], false);
        assert_eq!(scale, 0.2);
        assert_eq!(alpha, 10.0 / 255.0);
        let (pull, scale, _) = ForceRefractionState::advance(None, 1000, [1.0; 3], true);
        assert_eq!(scale, 0.2);
        assert_eq!(ForceRefractionState::advance(Some(pull), 1400, [2.0; 3], true).1, 1.0);
        let (rewind, scale, _) = ForceRefractionState::advance(Some(fixed), 500, [5.0; 3], false);
        assert_eq!(rewind.start, 500);
        assert_eq!(rewind.origin, [5.0; 3]);
        assert_eq!(scale, 1.0);
    }

    #[test]
    fn predator_uses_interruptible_damped_transition_and_rewinds() {
        let (start, visual) = CloakState::advance_predator(None, true, 1000);
        assert_eq!(visual, CloakVisual::Predator { progress: 0.0 });
        let (up, visual) = CloakState::advance_predator(Some(start), true, 1100);
        assert!(matches!(visual, CloakVisual::Predator { progress } if progress > 0.0 && progress < 1.0));
        let (down, visual) = CloakState::advance_predator(Some(up), false, 1200);
        // The critically damped velocity is preserved when the target flips.
        assert!(matches!(visual, CloakVisual::Predator { progress } if progress > 0.0));
        assert!(!down.cloaked);
        let (rewound, visual) = CloakState::advance_predator(Some(up), false, 400);
        assert_eq!(visual, CloakVisual::Normal);
        assert_eq!(rewound.predator_progress, 0.0);
    }

    #[test]
    fn cloak_tracks_two_second_taystjk_crossfade() {
        let (initial, visual) = CloakState::advance(None, false, 100);
        assert_eq!(visual, CloakVisual::Normal);
        let (on, visual) = CloakState::advance(Some(initial), true, 200);
        assert_eq!(visual, CloakVisual::Transition { shell: 0.0, skin: 1.0 });
        let (mid, visual) = CloakState::advance(Some(on), true, 1200);
        assert_eq!(visual, CloakVisual::Transition { shell: 0.5, skin: 0.5 });
        let (cloaked, visual) = CloakState::advance(Some(mid), true, 2200);
        assert_eq!(visual, CloakVisual::Cloaked);
        let (off, visual) = CloakState::advance(Some(cloaked), false, 2300);
        assert_eq!(visual, CloakVisual::Transition { shell: 1.0, skin: 0.0 });
        let (_, visual) = CloakState::advance(Some(off), false, 4300);
        assert_eq!(visual, CloakVisual::Normal);
    }

    #[test]
    fn cloak_reset_handles_time_rewind_and_first_cloaked_snapshot() {
        let (state, visual) = CloakState::advance(None, true, 3500);
        assert_eq!(visual, CloakVisual::Transition { shell: 0.0, skin: 1.0 });
        let (_, visual) = CloakState::advance(Some(state), true, 100);
        assert_eq!(visual, CloakVisual::Transition { shell: 0.0, skin: 1.0 });
    }
}

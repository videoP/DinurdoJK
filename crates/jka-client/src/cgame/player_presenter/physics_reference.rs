//! Opt-in independent, snapshot-driven 60 Hz reference for corpse physics.
//! No predicted pose, wall clock, live ragdoll, background-cut lane or FX output.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);
pub(crate) fn physics_reference_enabled() -> bool { ENABLED.load(Ordering::Relaxed) }
pub(crate) fn set_physics_reference_enabled(value: bool) { ENABLED.store(value, Ordering::Relaxed); }
const HZ: i64 = 60;
const MS: i64 = 1000;

pub(super) struct PhysicsReference {
    presenter: PlayerPresenter,
    next_tick: i64,
    last_snapshot_time: i32,
    server_count: i32,
    overlay: Option<DynamicModelSurface>,
    last_solved_tick: Option<i64>,
    last_report: Option<i32>,
}

fn first_tick(time: i32) -> i64 { -(-i64::from(time) * HZ).div_euclid(MS) }

impl PlayerPresenter {
    /// Construct a second presenter on demand. Its assets use the same VFS and
    /// immutable shader definitions, but every mutable runtime is independent.
    pub(crate) fn prepare_physics_reference(&mut self, game: &ClientGameState) {
        if !physics_reference_enabled() || !self.dynamic_cuts_on() {
            self.physics_reference = None;
            return;
        }
        let Some(snap) = game.current_snapshot() else { return; };
        let count = snap.snap_flags as i32 & crate::cgame::SNAPFLAG_SERVERCOUNT as i32;
        if self.physics_reference.as_ref().is_some_and(|r| r.server_count != count
            || snap.server_time < r.last_snapshot_time
            || snap.server_time.saturating_sub(r.last_snapshot_time) > 1000) {
            self.physics_reference = None;
        }
        if self.physics_reference.is_some() { return; }
        let result = (|| -> Result<PhysicsReference, String> {
            let mut p = PlayerPresenter::new_with_shared_shaders(self.assets.fork(), self.pbr,
                self.shader_definitions_for(self.pbr))?;
            p.set_async_loading(false);
            p.set_skinning_mode(crate::ui::Ghoul2SkinningMode::Cpu);
            p.set_early_frustum_cull(false);
            p.set_rt_shadow_casters_enabled(false);
            p.set_footstep_level(0, false);
            p.set_ragdoll_config(RagdollConfig {
                hz: 60, max_substeps: 1, debris_enabled: false, breakables_enhanced: false,
                weapon_impulses: false, explosion_impulses: false, force_impulses: false,
                debug: false, stats: false, ..self.ragdolls.config()
            });
            p.set_physics_map_mesh(&self.physics_reference_map)?;
            p.dynamic_cuts.reference_inline = true;
            p.ragdolls.enable_fixed_reference();
            Ok(PhysicsReference { presenter: p, next_tick: first_tick(snap.server_time),
                last_snapshot_time: snap.server_time, server_count: count, overlay: None,
                last_solved_tick: None, last_report: None })
        })();
        match result {
            Ok(r) => self.physics_reference = Some(Box::new(r)),
            Err(error) => { eprintln!("PHYSICS REFERENCE: {error}"); set_physics_reference_enabled(false); }
        }
    }

    pub(crate) fn physics_reference_command(&mut self, command: Ghoul2ServerCommand) {
        if let Some(r) = self.physics_reference.as_mut() { r.presenter.apply_ghoul2_server_command(command); }
    }

    /// Call before consuming each snapshot pair, and once after transitions.
    /// A tick at nextSnap belongs to the new pair, never the old pair.
    /// Without nextSnap, do not extrapolate or invent future player inputs.
    pub(crate) fn advance_physics_reference(&mut self, game: &ClientGameState,
        siege_classes: &[jka_assets::siege::SiegeClassVisual], target: i32) {
        let Some(r) = self.physics_reference.as_mut() else { return; };
        let Some(snap) = game.current_snapshot() else { return; };
        let next = game.physics_reference_next_time();
        r.last_snapshot_time = snap.server_time;
        let start = i64::from(snap.server_time) * HZ;
        let end = next.map(|n| i64::from(n) * HZ);
        // A very large gap is not a reproducible input stream. Restart at the
        // following snapshot instead of doing an unbounded catch-up.
        if next.is_some_and(|n| n.saturating_sub(snap.server_time) > 1000) {
            self.physics_reference = None;
            return;
        }
        if r.next_tick * MS < start { r.next_tick = first_tick(snap.server_time); }
        let mut advanced = false;
        while r.next_tick * MS <= i64::from(target) * HZ {
            let units = r.next_tick * MS;
            if end.map_or(units > start, |end| units >= end) { break; }
            let time = units as f64 / HZ as f64;
            let now = units.div_euclid(HZ) as i32;
            let Ok(mut entities) = game.physics_reference_entities(time) else { break; };
            let Some(ps) = game.physics_reference_player_state(time) else { break; };
            let local = crate::cgame::presented_player_state_entity(&ps);
            let crouched = ps.field_i32("pm_flags").unwrap_or(0)
                & (jka_movement::PMF_DUCKED | jka_movement::PMF_ROLLING) != 0;
            let height = if crouched { ps.field_i32("crouchheight").filter(|v| *v > 0).unwrap_or(16) }
                else { ps.field_i32("standheight").filter(|v| *v > 0).unwrap_or(40) } as f32;
            let p = &mut r.presenter;
            p.prepare_corpse_player_contacts(&entities, local.as_ref(), height,
                ps.field_i32("pm_type").unwrap_or(0) < 2, now);
            p.ragdolls.begin_reference_tick(now);
            // The local player is synthesized from unpredicted playerState,
            // and goes through the same full world pose path as every other player.
            if let Some(local) = local { entities.push(local); }
            entities.sort_unstable_by_key(|e| e.number);
            let _ = p.present_snapshot_players(&entities, game, siege_classes, now,
                None, None, None, None);
            p.finish_dynamic_cuts(now);
            // Reference outputs must never enter the live FX or audio queues.
            p.drain_fx_requests(); p.drain_cosmetic_draws(); p.drain_emote_fx();
            p.drain_footsteps(); p.drain_jk2_footsteps();
            r.last_solved_tick = Some(r.next_tick);
            r.next_tick += 1;
            advanced = true;
        }
        if advanced { r.overlay = r.presenter.ragdolls.reference_wireframe(); }
    }

    pub(crate) fn physics_reference_overlay(&mut self, live_time: i32) -> Option<DynamicModelSurface> {
        let stats = self.ragdolls.config().stats;
        let r = self.physics_reference.as_mut()?;
        if stats && r.last_report.is_none_or(|last| live_time.saturating_sub(last) >= 1000) {
            r.last_report = Some(live_time);
            if let Some(tick) = r.last_solved_tick {
                let time = tick as f64 * MS as f64 / HZ as f64;
                let (corpses, pieces, contacts) = r.presenter.ragdolls.reference_counts();
                println!("PHYSICS REFERENCE: tick={tick} serverTime={time:.3} liveTime={live_time} lagMs={:.3} corpses={corpses} pieces={pieces} contacts={contacts}", f64::from(live_time) - time);
            }
        }
        r.overlay.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absolute_reference_clock() {
        assert_eq!(first_tick(0), 0);
        assert_eq!(first_tick(1), 1);
        assert_eq!(first_tick(50), 3);
        assert_eq!(first_tick(51), 4);
        assert_eq!(first_tick(1000), 60);
        assert_eq!(first_tick(-50), -3);
        for ms in -1000..1000 {
            let tick = first_tick(ms);
            assert!(tick * MS >= i64::from(ms) * HZ);
            assert!((tick - 1) * MS < i64::from(ms) * HZ);
        }
    }
}

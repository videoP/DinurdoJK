//! Player presentation equivalent to TaystJK `CG_InterpolatePlayerState`.
use jka_protocol::server::{PlayerState, Snapshot};

/// Snapshot time, rather than commandTime, drives player presentation. The
/// caller has already advanced snapshots to bracket this presentation time.
pub(crate) fn interpolate_player_state_at(
    current: &Snapshot,
    next: Option<&Snapshot>,
    presentation_time_ms: f64,
) -> PlayerState {
    let next = next.filter(|next| {
        next.server_time > current.server_time && !super::snapshot_discontinuity(current, next)
    });
    let fraction = next.map_or(0.0, |next| {
        ((presentation_time_ms - f64::from(current.server_time))
            / f64::from(next.server_time - current.server_time)) as f32
    });
    interpolate_player_state(
        &current.player_state,
        next.map(|next| &next.player_state),
        fraction,
    )
}

/// TaystJK's third-person camera clock while following another player:
/// `predictedPlayerState.commandTime + predictedTimeFrac`, where
/// CG_InterpolatePlayerState sets the fraction to `f * (next.commandTime -
/// snap.commandTime)`. It is sub-millisecond, ignores server-clock drift, and
/// stands still while the followed client sends no commands. With no usable
/// nextSnap the fraction is 0 (cg_snapshot.c). A backwards commandTime never
/// rewinds the clock.
pub(crate) fn follow_command_clock_ms(
    current: &Snapshot,
    next: Option<&Snapshot>,
    presentation_time_ms: f64,
) -> f64 {
    let base = f64::from(current.player_state.field_i32("commandTime").unwrap_or(0));
    let Some(next) = next.filter(|next| {
        next.server_time > current.server_time && !super::snapshot_discontinuity(current, next)
    }) else {
        return base;
    };
    let fraction = (presentation_time_ms - f64::from(current.server_time))
        / f64::from(next.server_time - current.server_time);
    let gap = next.player_state.field_i32("commandTime").unwrap_or(0)
        .wrapping_sub(current.player_state.field_i32("commandTime").unwrap_or(0));
    base + fraction * f64::from(gap.max(0))
}

/// Clone the entire current state and interpolate only TaystJK's four fields.
/// Input-owned angles are applied before publishing by the session, leaving
/// viewheight, commandTime, animations, stats, events and vehicle fields intact.
pub(crate) fn interpolate_player_state(
    current: &PlayerState,
    next: Option<&PlayerState>,
    fraction: f32,
) -> PlayerState {
    let mut out = current.clone();
    let Some(next) = next else { return out };
    let bob = current.field_i32("bobCycle").unwrap_or(0);
    let mut next_bob = next.field_i32("bobCycle").unwrap_or(bob);
    if next_bob < bob {
        next_bob += 256;
    }
    // C converts float to int by truncation. Do not mask to a byte: TaystJK
    // returns 258 for 250 -> 5 at f=.75 and consumers mask when deriving phase.
    out.set_field_bits(
        "bobCycle",
        (bob as f32 + fraction * (next_bob - bob) as f32) as i32 as u32,
    );
    for axis in 0..3 {
        for field in ["origin", "velocity", "viewangles"] {
            let name = format!("{field}[{axis}]");
            if let (Some(from), Some(to)) = (current.field_f32(&name), next.field_f32(&name)) {
                let value = if field == "viewangles" {
                    super::lerp_angle(from, to, fraction)
                } else {
                    from + fraction * (to - from)
                };
                out.set_field_bits(&name, value.to_bits());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(time: i32) -> Snapshot {
        Snapshot {
            server_time: time,
            message_num: time,
            delta_num: -1,
            snap_flags: 0,
            ping: 0,
            server_command_num: 0,
            area_mask: [0; 32],
            player_state: PlayerState::default(),
            vehicle_player_state: None,
            entities: vec![],
        }
    }

    #[test]
    fn tayst_bob_wrap_truncates_without_masking() {
        let mut current = PlayerState::default();
        let mut next = current.clone();
        current.set_field_bits("bobCycle", 250);
        next.set_field_bits("bobCycle", 5);
        for (f, expected) in [(0.0, 250), (0.25, 252), (0.5, 255), (0.75, 258), (1.0, 261)] {
            assert_eq!(
                interpolate_player_state(&current, Some(&next), f).field_i32("bobCycle"),
                Some(expected)
            );
        }
        current.set_field_bits("bobCycle", 5);
        next.set_field_bits("bobCycle", 10);
        assert_eq!(
            interpolate_player_state(&current, Some(&next), 0.5).field_i32("bobCycle"),
            Some(7)
        );
    }

    #[test]
    fn only_tayst_fields_change_and_viewheight_stays_current() {
        let mut current = snapshot(100);
        current.player_state.stats[0] = 100;
        current.player_state.persistant[3] = 2;
        current.player_state.ammo[3] = 27;
        current.player_state.powerups[1] = 999;
        current.player_state.set_field_bits("viewheight", 26);
        current.player_state.set_field_bits("commandTime", 95);
        current.player_state.set_field_bits("events[0]", 17);
        current.player_state.set_field_bits("legsAnim", 123);
        let mut next = current.clone();
        next.server_time = 140;
        next.player_state.stats[0] = 50;
        next.player_state.set_field_bits("viewheight", 12);
        next.player_state.set_field_bits("commandTime", 135);
        next.player_state.set_field_bits("legsAnim", 456);
        next.player_state.set_field_bits("events[0]", 19);
        next.player_state
            .set_field_bits("origin[0]", 80.0f32.to_bits());
        next.player_state
            .set_field_bits("velocity[1]", 400.0f32.to_bits());
        next.player_state
            .set_field_bits("viewangles[1]", 90.0f32.to_bits());
        let ps = interpolate_player_state_at(&current, Some(&next), 110.0);
        let mut expected = current.player_state.clone();
        expected.set_field_bits("origin[0]", 20.0f32.to_bits());
        expected.set_field_bits("velocity[1]", 100.0f32.to_bits());
        expected.set_field_bits("viewangles[1]", 22.5f32.to_bits());
        assert_eq!(ps, expected);
        let entity = crate::cgame::presented_player_state_entity(&ps).unwrap();
        assert_eq!(entity.origin, [20.0, 0.0, 0.0]);
        assert_eq!(entity.angles, [0.0, 22.5, 0.0]);
        assert_eq!(entity.state.field_f32("pos.trDelta[1]"), Some(100.0));
    }

    #[test]
    fn angle_wrap_and_exact_half_turn_match_lerp_angle() {
        let mut current = PlayerState::default();
        let mut next = current.clone();
        for (from, to, expected) in [
            (350.0f32, 10.0f32, 360.0f32),
            (10.0, 350.0, 0.0),
            (0.0, 180.0, 90.0),
            (0.0, -180.0, -90.0),
        ] {
            current.set_field_bits("viewangles[1]", from.to_bits());
            next.set_field_bits("viewangles[1]", to.to_bits());
            assert_eq!(
                interpolate_player_state(&current, Some(&next), 0.5).field_f32("viewangles[1]"),
                Some(expected)
            );
        }
    }

    #[test]
    fn no_next_or_non_increasing_time_holds_entire_current_state() {
        let current = snapshot(100);
        let mut next = snapshot(100);
        next.player_state
            .set_field_bits("origin[0]", 1000.0f32.to_bits());
        assert_eq!(
            interpolate_player_state_at(&current, None, 150.0),
            current.player_state
        );
        for time in [100, 90] {
            next.server_time = time;
            assert_eq!(
                interpolate_player_state_at(&current, Some(&next), 150.0),
                current.player_state
            );
        }
    }

    #[test]
    fn teleport_follow_switch_and_server_restart_hold_until_transition() {
        let current = snapshot(100);
        let mut next = snapshot(150);
        next.player_state
            .set_field_bits("origin[0]", 10000.0f32.to_bits());
        next.player_state
            .set_field_bits("eFlags", super::super::EF_TELEPORT_BIT as u32);
        assert_eq!(
            interpolate_player_state_at(&current, Some(&next), 125.0),
            current.player_state
        );
        next.player_state.set_field_bits("eFlags", 0);
        next.player_state.set_field_bits("clientNum", 7);
        assert_eq!(
            interpolate_player_state_at(&current, Some(&next), 125.0),
            current.player_state
        );
        next.player_state.set_field_bits("clientNum", 0);
        next.snap_flags = super::super::SNAPFLAG_SERVERCOUNT;
        assert_eq!(
            interpolate_player_state_at(&current, Some(&next), 125.0),
            current.player_state
        );
        // Once next becomes current, hold its new position without a long lerp.
        assert_eq!(
            interpolate_player_state_at(&next, None, 150.0),
            next.player_state
        );
    }

    #[test]
    fn follow_clock_is_fractional_command_time_not_server_time() {
        let mut current = snapshot(100);
        current.player_state.set_field_bits("commandTime", 90);
        let mut next = snapshot(150);
        next.player_state.set_field_bits("commandTime", 98); // followed client ran 8 ms
        // No nextSnap: predictedTimeFrac is 0, the clock holds at commandTime.
        assert_eq!(follow_command_clock_ms(&current, None, 130.0), 90.0);
        // Halfway through the 50 ms server gap = halfway through the 8 ms command gap.
        assert!((follow_command_clock_ms(&current, Some(&next), 125.25) - 94.04).abs() < 1e-9);
        // Continuous at the snapshot hand-off: f = 1 equals the next commandTime.
        assert_eq!(follow_command_clock_ms(&current, Some(&next), 150.0), 98.0);
        assert_eq!(follow_command_clock_ms(&next, None, 150.0), 98.0);
        // A client restart (commandTime going backwards) never rewinds the clock.
        next.player_state.set_field_bits("commandTime", 10);
        assert_eq!(follow_command_clock_ms(&current, Some(&next), 125.0), 90.0);
    }

    #[test]
    fn fractional_presentation_time_is_not_quantized_to_milliseconds() {
        let current = snapshot(100);
        let mut next = snapshot(150);
        next.player_state
            .set_field_bits("origin[0]", 100.0f32.to_bits());
        assert_eq!(
            interpolate_player_state_at(&current, Some(&next), 100.25).field_f32("origin[0]"),
            Some(0.5)
        );
    }
}

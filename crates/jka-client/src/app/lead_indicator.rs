//! TaystJK CG_LeadIndicator numerical core (cg_draw.c / cg_weapons.c).
//! Screen projection and occlusion are intentionally owned by their existing
//! render-thread/30Hz app-thread paths, never by this pure math helper.

/// Return (projectile speed in units/s, uses TR_GRAVITY). Only weapons which
/// TaystJK CG_GetBulletSpeed handles contribute to a lead marker.
/// `tribes` is jaPRO jcinfo2 WTTRIBES (1<<4); projectile sniper is
/// jaPRO jcinfo PROJSNIPER (1<<29), defined in TaystJK bg_public.h.
pub(super) fn bullet_profile(weapon: i32, alt: bool, tribes: bool, jcinfo: i32) -> (f32, bool) {
    let speed = match weapon {
        4 => 1600,
        5 => if tribes { 10440 } else { 2300 },
        6 => if jcinfo & (1 << 29) != 0 { if tribes { 32000 } else { 9000 } } else { 0 },
        7 => if tribes { 3040 } else { 1300 },
        8 => if tribes { if alt { 2000 } else { 10440 } }
             else if alt { 1100 } else { 1600 },
        9 => if alt { 0 } else { 1800 },
        10 => if alt { if tribes { 2000 } else { 1050 } } else { 3500 },
        11 => if tribes { 3040 } else if alt { 450 } else { 900 },
        12 => if tribes { 1800 } else { 550 },
        13 => 550,
        15 => if alt { 0 } else if tribes { 3040 } else { 3000 },
        16 => if tribes { 10440 } else { 1600 },
        _ => 0,
    };
    let gravity = match weapon {
        8 | 10 => alt,
        6 => jcinfo & (1 << 29) != 0 || alt, // TaystJK gravity has no break before WP_REPEATER
        7 | 11 => tribes,
        13 => true,
        _ => false,
    };
    (speed as f32, gravity)
}

/// Match TaystJK's actual computation, including its historical latency
/// scaling quirk: it multiplies target velocity by (1 + ping_in_seconds),
/// rather than adding ping seconds to time to intercept.
/// The constant 800 is TaystJK's hardcoded DEFAULT_GRAVITY, not sv_gravity.
pub(super) fn predict_taystjk(
    viewer: [f32; 3], origin: [f32; 3], velocity: [f32; 3],
    speed: f32, gravity: bool, latency_ms: f32,
) -> Option<[f32; 3]> {
    if !speed.is_finite() || speed <= 0.0
        || !viewer.iter().chain(origin.iter()).chain(velocity.iter()).all(|v| v.is_finite())
    { return None; }
    if !gravity && velocity.iter().all(|v| *v == 0.0) { return None; }
    let distance = ((origin[0]-viewer[0]).powi(2)
        + (origin[1]-viewer[1]).powi(2)
        + (origin[2]-viewer[2]).powi(2)).sqrt();
    let eta = distance / speed;
    if !eta.is_finite() { return None; }
    let ping = (latency_ms * 0.001).clamp(0.0, 2.0);
    let mut lead = [0.0; 3];
    for axis in 0..3 { lead[axis] = origin[axis] + velocity[axis] * (1.0 + ping) * eta; }
    if gravity { lead[2] += 400.0 * eta * eta; }
    lead.iter().all(|v| v.is_finite()).then_some(lead)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standard_rocket_and_repeater_profiles_match_upstream() {
        assert_eq!(bullet_profile(11, false, false, 0), (900.0, false));
        assert_eq!(bullet_profile(11, true, false, 0), (450.0, false));
        assert_eq!(bullet_profile(8, true, false, 0), (1100.0, true));
        assert_eq!(bullet_profile(8, false, true, 0), (10440.0, false));
        assert_eq!(bullet_profile(3, false, false, 0), (0.0, false));
        assert_eq!(bullet_profile(6, false, false, 0), (0.0, false));
        assert_eq!(bullet_profile(6, false, false, 1 << 29), (9000.0, true));
        assert_eq!(bullet_profile(6, true, true, 1 << 29), (32000.0, true));
    }
    #[test]
    fn non_gravity_requires_motion_and_leads_by_eta() {
        assert_eq!(predict_taystjk([0.0;3],[900.0,0.0,0.0],[0.0;3],900.0,false,0.0),None);
        let lead=predict_taystjk([0.0;3],[900.0,0.0,0.0],[0.0,100.0,0.0],900.0,false,0.0).unwrap();
        assert_eq!(lead,[900.0,100.0,0.0]);
    }
    #[test]
    fn gravity_shows_static_targets() {
        let lead=predict_taystjk([0.0;3],[900.0,0.0,0.0],[0.0;3],900.0,true,0.0).unwrap();
        assert_eq!(lead,[900.0,0.0,400.0]);
    }
}

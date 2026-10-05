use crate::{Real, Vec3};

/// Signed dihedral angle and its analytic position gradients.
/// Faces are (0,1,2) and (1,0,3); a flat, consistently wound pair has angle 0.
/// Returns None on degenerate geometry. The closed-form gradients use the
/// shared edge and face normals; no inverse trig finite differences are used.
#[inline]
pub fn angle_and_gradients(p: [Vec3; 4]) -> Option<(Real, [Vec3; 4])> {
    let e = p[1] - p[0];
    let u = p[2] - p[0];
    let v = p[3] - p[0];
    let raw_a = e.cross(u);
    let raw_b = v.cross(e);
    let le = e.length();
    let la = raw_a.length();
    let lb = raw_b.length();
    if [le, la, lb]
        .iter()
        .any(|n| !n.is_finite() || *n <= Real::MIN_POSITIVE)
    {
        return None;
    }
    let t = e / le;
    let a = raw_a / la;
    let b = raw_b / lb;
    let s = t.dot(a.cross(b));
    let c = a.dot(b);
    // Opposite-vertex derivatives are the signed normals divided by triangle
    // altitude. Edge derivatives follow from the projections onto that edge.
    let gu = a * (-le / la);
    let gv = b * (-le / lb);
    let ge = -(gu * (u.dot(t) / le) + gv * (v.dot(t) / le));
    let gradients = [-ge - gu - gv, ge, gu, gv];
    if gradients.iter().any(|g| !g.is_finite()) {
        return None;
    }
    Some((s.atan2(c), gradients))
}

/// Signed angle without constructing gradients (for final-state diagnostics).
#[inline]
pub fn angle(p: [Vec3; 4]) -> Option<Real> {
    let e = p[1] - p[0];
    let a = e.cross(p[2] - p[0]);
    let b = (p[3] - p[0]).cross(e);
    let lengths = [e.length(), a.length(), b.length()];
    if lengths
        .iter()
        .any(|n| !n.is_finite() || *n <= Real::MIN_POSITIVE)
    {
        return None;
    }
    let t = e / lengths[0];
    let a = a / lengths[1];
    let b = b / lengths[2];
    Some(t.dot(a.cross(b)).atan2(a.dot(b)))
}

/// Difference of angles with the branch cut at +/- pi handled consistently.
#[inline]
pub fn angle_difference(angle: Real, rest: Real) -> Real {
    let d = angle - rest;
    // Most hinges stay away from the branch cut. Keep the general trig path
    // for wrapped, non-finite and boundary inputs, including f32's rounded pi.
    if d.abs() < std::f64::consts::PI as Real {
        d
    } else {
        d.sin().atan2(d.cos())
    }
}

/// Bound a dihedral Newton correction to the region where its gradient is
/// locally meaningful. Thin/folded faces can have rapidly changing gradients;
/// accepting the full scalar update can otherwise launch vertices metres away.
/// Scale the multiplier update together with displacement to retain XPBD state.
pub fn bounded_delta(
    p: [Vec3; 4],
    gradients: [Vec3; 4],
    weights: [Real; 4],
    delta: Real,
) -> (Real, bool) {
    let edge = p[1] - p[0];
    let length = edge.length();
    let altitude_a = edge.cross(p[2] - p[0]).length() / length;
    let altitude_b = (p[3] - p[0]).cross(edge).length() / length;
    let limit = 0.2 * length.min(altitude_a).min(altitude_b);
    let largest = gradients
        .iter()
        .zip(weights)
        .map(|(gradient, weight)| gradient.length() * weight * delta.abs())
        .fold(0.0 as Real, Real::max);
    if largest > limit {
        (delta * (limit / largest), true)
    } else {
        (delta, false)
    }
}

#[cfg(test)]
mod bounded_step_tests {
    use super::*;

    #[test]
    fn ordinary_small_bend_steps_are_unchanged() {
        let p = [Vec3::ZERO, Vec3::X, Vec3::Y, -Vec3::Y];
        let (_, g) = angle_and_gradients(p).unwrap();
        let (delta, limited) = bounded_delta(p, g, [1.0; 4], 0.001);
        assert_eq!(delta, 0.001);
        assert!(!limited);
    }

    #[test]
    fn thin_fold_caps_the_accepted_step_and_preserves_sign() {
        let p = [
            Vec3::ZERO,
            Vec3::new(0.1, 0.0, 0.0),
            Vec3::new(0.05, 0.00001, 0.0),
            Vec3::new(0.05, -0.00001, 0.000005),
        ];
        let (_, g) = angle_and_gradients(p).unwrap();
        let weights = [0.0, 0.0, 5000.0, 5000.0];
        for proposed in [-1000.0, 1000.0] {
            let (delta, limited) = bounded_delta(p, g, weights, proposed);
            assert!(limited);
            assert_eq!(delta.signum(), proposed.signum());
            for (gradient, weight) in g.iter().zip(weights) {
                assert!(gradient.length() * weight * delta.abs() <= 0.0000021);
            }
        }
    }

    #[test]
    fn the_bound_scales_with_geometry() {
        let p = [Vec3::ZERO, Vec3::X, Vec3::Y, -Vec3::Y];
        let (_, g) = angle_and_gradients(p).unwrap();
        let (a, limited_a) = bounded_delta(p, g, [1.0; 4], 100.0);
        let large = p.map(|p| p * 10.0);
        let (_, large_g) = angle_and_gradients(large).unwrap();
        let (b, limited_b) = bounded_delta(large, large_g, [1.0; 4], 10000.0);
        assert!(limited_a && limited_b);
        assert!((b / a - 100.0).abs() < 0.001);
    }
}

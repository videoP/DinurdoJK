use crate::{Real, Vec3};

pub fn project(
    p: &mut [Vec3],
    w: &[Real],
    ids: [u32; 2],
    rest: Real,
    alpha: Real,
    lambda: &mut Real,
) -> bool {
    let [a, b] = ids.map(|i| i as usize);
    let d = p[b] - p[a];
    let len = d.length();
    if !len.is_finite() || len <= Real::MIN_POSITIVE {
        return false;
    }
    let denom = w[a] + w[b] + alpha;
    if denom <= 0.0 {
        return true;
    }
    let dl = (-(len - rest) - alpha * (*lambda)) / denom;
    *lambda += dl;
    let grad = d / len;
    p[a] -= grad * (w[a] * dl);
    p[b] += grad * (w[b] * dl);
    true
}

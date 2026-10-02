//! Frame-constant cloud wind terms.
//!
//! The cloud shader used to rebuild these from `time` in every density sample:
//! the gust and veer waves, their closed-form integrals and the slow erosion
//! drift, some twenty transcendental calls per sample. A frame of clouds takes
//! several million samples (the primary march, its self-shadow taps, and the
//! per-pixel ground cloud shadow), and none of that depends on the sample, only
//! on the frame time. They are evaluated once here, in f64 so a long session's
//! large `time` does not quantise the waves, and handed to the shader through
//! the post uniform.

/// Wind inputs, exactly as the cloud shader received them in its uniform.
#[derive(Clone, Copy, Debug)]
pub struct CloudWind {
    /// Base direction, radians in the world XZ plane.
    pub angle: f32,
    /// Base speed, world units per second.
    pub speed: f32,
    /// Gust strength (speed variation), nominally 0..1.
    pub gust: f32,
    /// Veer amplitude (direction variation), radians.
    pub shift: f32,
}

/// Everything time-dependent the cloud shader needs, one `[x, y, z, w]` each.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CloudWindTerms {
    /// Unit wind direction now.
    pub direction: [f32; 4],
    /// Accumulated advection of the cloud field now.
    pub offset: [f32; 4],
    /// Advection between the previous frame and now (for history reprojection).
    pub delta: [f32; 4],
    /// Slow drift of the erosion volume relative to the weather field.
    pub detail_slip: [f32; 4],
    /// Unit-scale wandering of the erosion volume; the shader scales it by tile.
    pub detail_billow: [f32; 4],
}

fn direction_at(wind: CloudWind, time: f64) -> [f64; 3] {
    let shift_wave = (time * 0.13).sin() * 0.7 + (time * 0.047 + 2.4).sin() * 0.3;
    let angle = f64::from(wind.angle) + f64::from(wind.shift) * shift_wave;
    [angle.cos(), 0.0, angle.sin()]
}

fn speed_at(wind: CloudWind, time: f64) -> f64 {
    let gust_wave = (time * 0.83).sin() * 0.65 + (time * 1.71 + 1.9).sin() * 0.35;
    f64::from(wind.speed).max(0.0) * (1.0 + f64::from(wind.gust) * gust_wave).max(0.0)
}

/// Continuous advection: speed variation is integrated exactly, and angular
/// wandering uses a bounded first-order lateral integral so a changing
/// direction does not multiply by the whole elapsed runtime and make the cloud
/// field jump.
fn offset_at(wind: CloudWind, time: f64) -> [f64; 3] {
    let angle = f64::from(wind.angle);
    let dir = [angle.cos(), 0.0, angle.sin()];
    let perp = [-dir[2], 0.0, dir[0]];
    let speed = f64::from(wind.speed).max(0.0);
    let gust = f64::from(wind.gust).clamp(0.0, 1.0);
    let shift = f64::from(wind.shift).clamp(0.0, std::f64::consts::PI);

    let gust_integral = 0.65 * (1.0 - (time * 0.83).cos()) / 0.83
        + 0.35 * (1.9f64.cos() - (time * 1.71 + 1.9).cos()) / 1.71;
    let shift_integral = 0.7 * (1.0 - (time * 0.13).cos()) / 0.13
        + 0.3 * (2.4f64.cos() - (time * 0.047 + 2.4).cos()) / 0.047;
    let along = time + gust * gust_integral;
    let side = shift.min(1.2) * shift_integral;
    [
        speed * (dir[0] * along + perp[0] * side),
        0.0,
        speed * (dir[2] * along + perp[2] * side),
    ]
}

fn vec4(v: [f64; 3]) -> [f32; 4] {
    [v[0] as f32, v[1] as f32, v[2] as f32, 0.0]
}

/// `previous_time` equals `time` when there is no valid history to reproject.
pub fn terms(wind: CloudWind, time: f32, previous_time: f32) -> CloudWindTerms {
    let t = f64::from(time);
    let now = offset_at(wind, t);
    let before = offset_at(wind, f64::from(previous_time));
    let dir = direction_at(wind, t);
    let perp = [-dir[2], 0.0, dir[0]];
    let drift = speed_at(wind, t).max(24.0) * t;
    CloudWindTerms {
        direction: vec4(dir),
        offset: vec4(now),
        delta: vec4([now[0] - before[0], 0.0, now[2] - before[2]]),
        detail_slip: vec4([
            (perp[0] * 0.075 + dir[0] * 0.022) * drift,
            0.0,
            (perp[2] * 0.075 + dir[2] * 0.022) * drift,
        ]),
        detail_billow: vec4([
            (t * 0.052).sin() + 0.35 * (t * 0.019 + 1.1).sin(),
            (t * 0.043 + 2.1).sin(),
            (t * 0.047 + 0.8).cos() + 0.30 * (t * 0.023).sin(),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wind() -> CloudWind {
        CloudWind { angle: 0.6, speed: 40.0, gust: 0.5, shift: 0.0 }
    }

    #[test]
    fn advection_starts_at_rest_and_history_delta_is_the_difference() {
        let at_zero = terms(wind(), 0.0, 0.0);
        assert_eq!(at_zero.offset, [0.0; 4]);
        let now = terms(wind(), 12.5, 12.0);
        let earlier = terms(wind(), 12.0, 12.0);
        for axis in [0, 2] {
            let expected = now.offset[axis] - earlier.offset[axis];
            assert!((now.delta[axis] - expected).abs() < 1e-3, "axis {axis}");
        }
        assert_eq!(terms(wind(), 9.0, 9.0).delta, [0.0; 4], "no valid history means no reprojection shift");
    }

    /// The offset must be the exact time integral of the gusting wind speed
    /// along the base direction (with veer off), which is the property that
    /// keeps clouds from jumping when gusts change.
    #[test]
    fn offset_is_the_integral_of_the_gusting_speed() {
        let wind = wind();
        let h = 1e-3;
        for time in [0.0f64, 3.0, 47.5, 900.0] {
            let a = offset_at(wind, time - h);
            let b = offset_at(wind, time + h);
            let speed = ((b[0] - a[0]).powi(2) + (b[2] - a[2]).powi(2)).sqrt() / (2.0 * h);
            assert!((speed - speed_at(wind, time)).abs() < 1e-3 * speed.max(1.0), "t={time}: {speed} vs {}", speed_at(wind, time));
        }
    }

    #[test]
    fn direction_is_unit_and_billow_stays_bounded() {
        let wind = CloudWind { shift: 0.8, ..wind() };
        for time in [0.0f32, 1.0, 250.0, 86_400.0] {
            let t = terms(wind, time, time);
            let d = t.direction;
            assert!(((d[0] * d[0] + d[2] * d[2]) - 1.0).abs() < 1e-5);
            assert!(t.detail_billow.iter().all(|c| c.abs() <= 1.4), "{:?}", t.detail_billow);
        }
    }
}

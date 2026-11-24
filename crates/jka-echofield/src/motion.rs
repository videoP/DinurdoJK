use crate::{
    geometry::Vec3,
    profile::{dot, length, scale, sub},
};

/// Doppler ratio in metres and seconds. Teleports and camera cuts have no
/// physical velocity and must not bend an active sound's pitch.
pub fn doppler_ratio(
    previous_listener: Vec3,
    listener: Vec3,
    previous_source: Vec3,
    source: Vec3,
    seconds: f64,
) -> f32 {
    const SPEED_OF_SOUND: f64 = 343.0;
    if !(0.0..=0.25).contains(&seconds) || seconds == 0.0 {
        return 1.0;
    }
    let direction = sub(source, listener);
    let distance = length(direction);
    let listener_delta = sub(listener, previous_listener);
    let source_delta = sub(source, previous_source);
    if distance < 0.05 || length(listener_delta) > 32.0 || length(source_delta) > 32.0 {
        return 1.0;
    }
    let unit = scale(direction, 1.0 / distance);
    let radial =
        |delta| (dot(delta, unit) / seconds).clamp(-0.8 * SPEED_OF_SOUND, 0.8 * SPEED_OF_SOUND);
    ((SPEED_OF_SOUND + radial(listener_delta)) / (SPEED_OF_SOUND + radial(source_delta)))
        .clamp(0.5, 2.0) as f32
}

//! Animated hit meshes and their accompanying airborne droplets.
use crate::{
    fx::system::{FxDraw, FxFrame},
    ninja_blood::{blood_pigment, Droplet},
    ninja_fire_debug::CM_PER_MAP_UNIT,
};
use glam::Vec3;

mod spurt;
pub(crate) use spurt::SpurtEffects;

pub(crate) fn append<'a>(frame: &mut FxFrame, drops: impl Iterator<Item = &'a Droplet>) {
    let color = blood_pigment(1.0).map(|channel| (channel * 255.0).round() as u8);
    for drop in drops {
        if drop.pos.is_finite() && drop.velocity.is_finite() {
            append_droplet(frame, drop, [color[0], color[1], color[2], 255]);
        }
    }
}

fn append_droplet(frame: &mut FxFrame, drop: &Droplet, color: [u8; 4]) {
    let p = drop.pos;
    let v = drop.velocity;
    let axis = v.try_normalize().unwrap_or(Vec3::Z);
    let helper = if axis.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
    let side = axis.cross(helper).normalize();
    let up = axis.cross(side);
    let radius = (drop.size_cm * 0.13 / CM_PER_MAP_UNIT).clamp(0.35, 1.4);
    let stretch = (1.0 + v.length() / 180.0).min(3.5);
    let axes = [side, up, -side, -up, axis, -axis];
    let positions = axes
        .iter()
        .enumerate()
        .map(|(i, n)| (p + *n * radius * if i >= 4 { stretch } else { 1.0 }).to_array())
        .collect();
    let normals = axes.map(|n| n.to_array()).to_vec();
    frame.draws.push(FxDraw::LitMesh {
        positions,
        normals,
        rgba: vec![color; 6],
        indices: vec![
            0, 1, 4, 1, 2, 4, 2, 3, 4, 3, 0, 4, 1, 0, 5, 2, 1, 5, 3, 2, 5, 0, 3, 5,
        ],
        lighting_origin: p.to_array(),
    });
}

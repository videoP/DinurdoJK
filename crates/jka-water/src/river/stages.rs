//! CPU reference for two translated GPU stages. The stencil is explicit so
//! resource sampling/boundary policy is tested separately from the equations.
//! No timestep scheduler or replacement hydraulic algorithm is provided here.

use super::{Geometry, Parameters, Water};

#[derive(Clone, Copy, Debug)]
pub struct VelocityStencil {
    pub water: Water,
    pub geometry: Geometry,
    pub left_water: Water,
    pub left_geometry: Geometry,
    pub down_water: Water,
    pub down_geometry: Geometry,
}

fn length(value: [f32; 2]) -> f32 {
    (value[0] * value[0] + value[1] * value[1]).sqrt()
}
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + t * (b - a)
}

/// Grid2D_SW_UpdateVelocity, upwind friction variant in the river system.
pub fn velocity(s: VelocityStencil, p: Parameters) -> [f32; 2] {
    let eps = 1e-8;
    let mut output = s.water.velocity;
    let mut masks = [s.left_geometry.collider, s.down_geometry.collider];
    let mut collision = [
        s.left_geometry.collision_velocity[0],
        s.down_geometry.collision_velocity[1],
    ];
    let neighbors = [
        (s.left_water, s.left_geometry),
        (s.down_water, s.down_geometry),
    ];
    for (axis, (water, geometry)) in neighbors.into_iter().enumerate() {
        if (geometry.bottom > s.water.height && water.depth < eps)
            || (s.geometry.bottom > water.height && s.water.depth < eps)
            || (water.depth < eps && s.water.depth < eps)
        {
            collision[axis] = 0.0;
            masks[axis] = 1.0;
        }
        output[axis] -= p.dt_over_dx() * p.gravity * (s.water.height - water.height);
        output[axis] = lerp(output[axis], collision[axis], masks[axis]);
        output[axis] = lerp(
            output[axis],
            s.geometry.collision_velocity[axis],
            s.geometry.collider,
        );
    }
    let speed = length(output);
    if speed > eps {
        let depth = [
            if output[0] >= 0.0 {
                s.left_water.depth
            } else {
                s.water.depth
            },
            if output[1] >= 0.0 {
                s.down_water.depth
            } else {
                s.water.depth
            },
        ];
        // Not Unreal: semi-implicit Manning friction (see shallow.wgsl). The
        // explicit force clamped at speed/dt zeroed steep-lip runoff.
        for axis in 0..2 {
            let resistance =
                p.dt * p.friction * speed / (depth[axis] / 100.0).powf(4.0 / 3.0).max(eps);
            output[axis] /= 1.0 + resistance;
        }
    }
    for component in &mut output {
        *component *= p.velocity_dissipation;
    }
    let speed = length(output);
    if speed > eps {
        for component in &mut output {
            *component = speed.clamp(0.0, p.max_velocity) * *component / speed;
        }
    }
    output
}

#[derive(Clone, Copy, Debug)]
pub struct DepthStencil {
    pub water: Water,
    pub geometry: Geometry,
    /// Left, right, down, up; then the same neighbors two cells away.
    pub depth: [f32; 4],
    pub depth_two: [f32; 4],
    pub right_velocity_x: f32,
    pub up_velocity_y: f32,
}

fn sign(value: f32) -> f32 {
    if value > 0.0 {
        1.0
    } else if value < 0.0 {
        -1.0
    } else {
        0.0
    }
}

fn slope(left: f32, right: f32) -> f32 {
    sign(left) * left.abs().min(sign(left) * right).max(0.0)
}

fn smoothstep(edge1: f32, value: f32) -> f32 {
    let t = (value / edge1).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Grid2D_SW_UpdateDepthGrid with ComplexUpwind, DepthLimiter, Upwind and
/// KeepWaterDepthInsideColliders enabled, as selected in the cached river shader.
pub fn depth(s: DepthStencil, p: Parameters) -> Water {
    let d = s.water.depth;
    let [left, right, down, up] = s.depth;
    let [left2, right2, down2, up2] = s.depth_two;
    let [vx, vy] = s.water.velocity;
    let vr = s.right_velocity_x;
    let vu = s.up_velocity_y;
    let mut integration = [
        if vx > 0.0 {
            left + 0.5 * slope(left - left2, d - left)
        } else {
            d - 0.5 * slope(d - left, right - d)
        },
        if vr > 0.0 {
            d + 0.5 * slope(d - left, right - d)
        } else {
            right - 0.5 * slope(right - d, right2 - right)
        },
        if vy > 0.0 {
            down + 0.5 * slope(down - down2, d - down)
        } else {
            d - 0.5 * slope(d - down, up - d)
        },
        if vu > 0.0 {
            d + 0.5 * slope(d - down, up - d)
        } else {
            up - 0.5 * slope(up - d, up2 - up)
        },
    ];
    let wet = s.depth.iter().filter(|&&value| value > 1e-8).count() as f32;
    // Preserve the source expression, including its all-dry 0/0 intermediate.
    // f32::max selects the numeric operand for NaN, matching this CPU reference's
    // max(0, adjustment). Cross-backend behavior is a validation requirement.
    let average = (left + right + down + up) / wet;
    let adjustment = (average - p.integration_depth_max * p.dx / (p.gravity * p.dt)).max(0.0);
    for value in &mut integration {
        *value -= adjustment;
    }
    let [il, ir, id, iu] = integration;
    let mut value = d - p.dt_over_dx() * ((ir * vr - il * vx) + (iu * vu - id * vy));
    value -= p.dt * p.fade_speed * (1.0 - smoothstep(p.dx * p.fade_start, value));
    value = lerp(value, d, s.geometry.collider)
        .max(0.0)
        .min(p.max_depth);
    Water {
        depth: value,
        height: s.geometry.bottom + value,
        ..s.water
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parameters() -> Parameters {
        let mut p = Parameters::cached_river_variant(100.0, 0.01, 980.0, 10000.0, 10000.0);
        p.friction = 0.0;
        p.velocity_dissipation = 1.0;
        p.fade_speed = 0.0;
        p
    }

    #[test]
    fn lake_at_rest_over_uneven_bed_stays_at_rest() {
        let water = Water {
            height: 300.0,
            depth: 200.0,
            velocity: [0.0; 2],
        };
        let left = Water {
            depth: 275.0,
            ..water
        };
        let down = Water {
            depth: 150.0,
            ..water
        };
        let v = velocity(
            VelocityStencil {
                water,
                geometry: Geometry {
                    bottom: 100.0,
                    ..Geometry::default()
                },
                left_water: left,
                left_geometry: Geometry {
                    bottom: 25.0,
                    ..Geometry::default()
                },
                down_water: down,
                down_geometry: Geometry {
                    bottom: 150.0,
                    ..Geometry::default()
                },
            },
            parameters(),
        );
        assert_eq!(v, [0.0; 2]);
    }

    #[test]
    fn pressure_gradient_has_expected_magnitude_and_direction() {
        let water = Water {
            height: 200.0,
            depth: 200.0,
            velocity: [0.0; 2],
        };
        let v = velocity(
            VelocityStencil {
                water,
                geometry: Geometry::default(),
                left_water: Water {
                    height: 210.0,
                    ..water
                },
                left_geometry: Geometry::default(),
                down_water: Water {
                    height: 190.0,
                    ..water
                },
                down_geometry: Geometry::default(),
            },
            parameters(),
        );
        assert!((v[0] - 0.98).abs() < 1e-6);
        assert!((v[1] + 0.98).abs() < 1e-6);
    }

    #[test]
    fn friction_cannot_reverse_velocity_even_at_large_timestep() {
        let water = Water {
            height: 1.0,
            depth: 1.0,
            velocity: [100.0, 0.0],
        };
        let mut p = parameters();
        p.friction = 10.0;
        p.dt = 1.0;
        let v = velocity(
            VelocityStencil {
                water,
                geometry: Geometry::default(),
                left_water: water,
                left_geometry: Geometry::default(),
                down_water: water,
                down_geometry: Geometry::default(),
            },
            p,
        );
        assert!(v[0] > 0.0 && v[0] < 1e-3);
        assert_eq!(v[1], 0.0);
    }

    #[test]
    fn thin_water_over_steep_lip_keeps_downhill_velocity() {
        // 1 cm of water on a 12 m lip, starting at rest. Unreal's explicit
        // friction clamp cancelled the whole pressure acceleration here.
        let p = Parameters::cached_river_variant(40.0, 1.0 / 60.0, 980.0, 2400.0, 6000.0);
        let high_geo = Geometry { bottom: 1200.0, ..Geometry::default() };
        let high = Water { height: 1201.0, depth: 1.0, velocity: [0.0; 2] };
        let low = Water::default();
        let predicted = p.dt_over_dx() * p.gravity * high.height;
        assert!(p.dt * p.friction * predicted / (high.depth / 100.0).powf(4.0 / 3.0) > 1.0);
        let v = velocity(
            VelocityStencil {
                water: low,
                geometry: Geometry::default(),
                left_water: high,
                left_geometry: high_geo,
                down_water: low,
                down_geometry: Geometry::default(),
            },
            p,
        );
        assert!(v[0] > 50.0 && v[0] < predicted);
        assert_eq!(v[1], 0.0);
    }

    #[test]
    fn uniform_flow_preserves_depth_and_height() {
        let water = Water {
            height: 350.0,
            depth: 250.0,
            velocity: [30.0, -20.0],
        };
        let next = depth(
            DepthStencil {
                water,
                geometry: Geometry {
                    bottom: 100.0,
                    ..Geometry::default()
                },
                depth: [250.0; 4],
                depth_two: [250.0; 4],
                right_velocity_x: 30.0,
                up_velocity_y: -20.0,
            },
            parameters(),
        );
        assert_eq!(next, water);
    }
}

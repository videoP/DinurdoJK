//! Selected source/sink and spline-matching operations from the main river's
//! cached shader. Original algorithms: Copyright Epic Games, Inc.
//! Positions/heights are relative to the simulation origin, in centimetres.

use super::{Geometry, Water};

#[derive(Clone, Copy, Debug, Default)]
pub struct WaterInfo {
    pub height: f32,
    pub depth: f32,
    pub flow_velocity: [f32; 2],
}

#[derive(Clone, Copy, Debug)]
pub struct SourceBox {
    pub position: [f32; 3],
    pub size: [f32; 3],
    /// Unreal's box angle in radians, around +Z. This is not map heading.
    pub angle: f32,
    pub sink: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Controls {
    /// Sink uses Engine.DeltaTime, while spline removal uses Emitter.SimDt.
    pub frame_dt: f32,
    pub sink_dissipation: f32,
    pub match_spline: bool,
    pub height_matching_amount: f32,
    pub remove_outside_amount: f32,
    /// Location of the first grid corner relative to the simulation origin.
    pub grid_min: [f32; 3],
}

impl Controls {
    pub fn cached_river_variant(frame_dt: f32, grid_min: [f32; 3]) -> Self {
        Self {
            frame_dt,
            sink_dissipation: 10.0,
            match_spline: true,
            height_matching_amount: 2.0,
            remove_outside_amount: 50.0,
            grid_min,
        }
    }
}

fn smoothstep(edge: f32, value: f32) -> f32 {
    let t = (value / edge).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn apply(
    mut water: Water,
    geo: Geometry,
    info: WaterInfo,
    position: [f32; 3],
    boxes: &[SourceBox],
    controls: Controls,
) -> Water {
    // The graph invokes the entire source array before the entire sink array.
    for sink in [false, true] {
        for source in boxes.iter().filter(|b| b.sink == sink) {
            let local: [f32; 3] = std::array::from_fn(|i| position[i] - source.position[i]);
            // Preserve the source's inverse quaternion formulation, including
            // half-angle trig, rather than retuning rotation at brush edges.
            let z = (source.angle / 2.0).sin();
            let w = -(source.angle / 2.0).cos();
            let rotated = [
                local[0] - 2.0 * z * local[1] * w - 2.0 * z * z * local[0],
                local[1] + 2.0 * z * local[0] * w - 2.0 * z * z * local[1],
                local[2],
            ];
            if !sink
                && (0..3).all(|i| {
                    rotated[i] >= -0.5 * source.size[i] && rotated[i] <= 0.5 * source.size[i]
                })
            {
                water.height = source.position[2] + 0.5 * source.size[2];
                water.depth = water.height - geo.bottom;
                // In this cached graph, the source's added velocity is (0,0),
                // nonadditive, and only replaces a smaller input magnitude.
            }
            if sink && water.depth > 1e-5 {
                let distance = rotated[1].abs();
                let radius = source.size[0];
                // Actual source behavior is a band: no X/Z box rejection here.
                if distance <= radius {
                    let target = if info.depth > 1e-5 {
                        info.height
                    } else {
                        geo.bottom
                    };
                    let blend = smoothstep(radius, distance);
                    let goal = target + blend * (water.height - target);
                    water.height +=
                        controls.sink_dissipation * (goal - water.height) * controls.frame_dt;
                    water.depth = water.height - geo.bottom;
                    for velocity in &mut water.velocity {
                        let goal = blend * *velocity;
                        *velocity += 0.1
                            * controls.sink_dissipation
                            * (goal - *velocity)
                            * controls.frame_dt;
                    }
                }
            }
        }
    }
    water
}

pub fn match_spline(
    mut water: Water,
    geo: Geometry,
    info: WaterInfo,
    sim_dt: f32,
    controls: Controls,
) -> Water {
    if controls.match_spline {
        let old_depth = water.depth;
        water.depth = (old_depth
            - sim_dt * controls.remove_outside_amount * (1.0 - smoothstep(0.1, info.depth)))
        .max(0.0);
        // The reference intentionally uses the pre-removal depth for height.
        water.height = geo.bottom + old_depth;
        if water.height < info.height && water.depth > 1e-5 && info.depth > 1e-5 {
            water.height = (water.height + controls.height_matching_amount).min(info.height);
            water.depth = water.height - geo.bottom;
        }
    }
    water
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_sets_stage_in_rotated_box_and_keeps_existing_velocity() {
        let source = SourceBox {
            position: [0.0; 3],
            size: [20.0, 4.0, 100.0],
            angle: std::f32::consts::FRAC_PI_2,
            sink: false,
        };
        let water = Water {
            velocity: [10.0, 20.0],
            ..Water::default()
        };
        let c = Controls::cached_river_variant(0.02, [0.0; 3]);
        let inside = apply(
            water,
            Geometry::default(),
            WaterInfo::default(),
            [0.0, 8.0, 0.0],
            &[source],
            c,
        );
        assert_eq!(
            (inside.height, inside.depth, inside.velocity),
            (50.0, 50.0, [10.0, 20.0])
        );
        assert_eq!(
            apply(
                water,
                Geometry::default(),
                WaterInfo::default(),
                [8.0, 0.0, 0.0],
                &[source],
                c
            ),
            water
        );
    }

    #[test]
    fn sink_is_the_reference_band_and_uses_frame_delta_instead_of_simulation_delta() {
        let sink = SourceBox {
            position: [0.0; 3],
            size: [20.0, 4.0, 100.0],
            angle: 0.0,
            sink: true,
        };
        let water = Water {
            height: 100.0,
            depth: 100.0,
            velocity: [10.0, 0.0],
        };
        let c = Controls::cached_river_variant(0.02, [0.0; 3]);
        let result = apply(
            water,
            Geometry::default(),
            WaterInfo::default(),
            [1000.0, 0.0, 0.0],
            &[sink],
            c,
        );
        assert_eq!((result.height, result.depth), (80.0, 80.0));
        assert!((result.velocity[0] - 9.8).abs() < 1e-6);
    }

    #[test]
    fn spline_removal_retains_reference_height_until_normal_stage_reset() {
        let water = Water {
            height: 10.0,
            depth: 10.0,
            velocity: [1.0, 2.0],
        };
        let c = Controls::cached_river_variant(0.1, [0.0; 3]);
        let result = match_spline(water, Geometry::default(), WaterInfo::default(), 0.02, c);
        assert_eq!((result.height, result.depth), (10.0, 9.0));
        let result = match_spline(
            water,
            Geometry::default(),
            WaterInfo {
                height: 11.0,
                depth: 11.0,
                ..WaterInfo::default()
            },
            0.02,
            c,
        );
        assert_eq!((result.height, result.depth), (11.0, 11.0));
    }
}

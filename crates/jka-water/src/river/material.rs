//! Native live-grid material operations and final normal-field blur.
//! See presentation-reference.json. Copyright Epic Games, Inc. All Rights Reserved.
//! These are material-function boundaries, not a complete water material.

pub const SHADER: &str = include_str!("material.wgsl");
pub const OUTPUT_SHADER: &str = include_str!("material_outputs.wgsl");

/// Half-quantized NormalGrid values must be supplied to this stage. Its output
/// is normalized before a separate RGBA16f render-target store. X-major loops
/// and length-based kernel weights follow cached Niagara stage 17.
pub fn blur_normals(
    dimensions: [usize; 2],
    normals: &[[f32; 4]],
) -> Result<Vec<[f32; 4]>, &'static str> {
    if dimensions.contains(&0) || dimensions[0].checked_mul(dimensions[1]) != Some(normals.len()) {
        return Err("invalid river material normal field");
    }
    let mut result = vec![[0.0; 4]; normals.len()];
    for y in 0..dimensions[1] {
        for x in 0..dimensions[0] {
            let mut sum = [0.0f32; 3];
            let mut total = 0.0f32;
            for xx in -1isize..=1 {
                for yy in -1isize..=1 {
                    let sx = (x as isize + xx).clamp(0, dimensions[0] as isize - 1) as usize;
                    let sy = (y as isize + yy).clamp(0, dimensions[1] as isize - 1) as usize;
                    let a = 1.0 - xx.abs() as f32 / 2.0;
                    let b = 1.0 - yy.abs() as f32 / 2.0;
                    let weight = (a * a + b * b).sqrt();
                    total += weight;
                    for c in 0..3 {
                        sum[c] += normals[sx + sy * dimensions[0]][c] * weight;
                    }
                }
            }
            if total > 1e-5 {
                for v in &mut sum {
                    *v /= total;
                }
            }
            let length = (sum[0] * sum[0] + sum[1] * sum[1] + sum[2] * sum[2]).sqrt();
            if length > 1e-5 {
                for v in &mut sum {
                    *v /= length;
                }
            }
            result[x + y * dimensions[0]] = [sum[0], sum[1], sum[2], 0.0];
        }
    }
    Ok(result)
}

/// Non-VT MF_ShallowWaterRiver_BilinearInterp, selected UseSample=false,
/// pinned mip level zero. Each native Load returns zero outside the texture.
pub fn live_field(
    dimensions: [usize; 2],
    values: &[[f32; 4]],
    uv: [f32; 2],
) -> Result<[f32; 4], &'static str> {
    if dimensions.contains(&0)
        || dimensions[0].checked_mul(dimensions[1]) != Some(values.len())
        || uv.iter().any(|v| !v.is_finite())
    {
        return Err("invalid river material sample");
    }
    let position = std::array::from_fn::<_, 2, _>(|i| uv[i] * dimensions[i] as f32 - 0.5);
    if position.iter().any(|v| !v.is_finite()) {
        return Err("river material UV overflow");
    }
    let cell = position.map(f32::floor);
    // Match OOB zero even for finite UVs too large for an integer conversion.
    let read = |x: f32, y: f32| {
        if x < 0.0 || y < 0.0 || x >= dimensions[0] as f32 || y >= dimensions[1] as f32 {
            [0.0; 4]
        } else {
            values[x as usize + y as usize * dimensions[0]]
        }
    };
    let a = read(cell[0], cell[1]);
    let b = read(cell[0] + 1.0, cell[1]);
    let c = read(cell[0], cell[1] + 1.0);
    let d = read(cell[0] + 1.0, cell[1] + 1.0);
    let fraction = std::array::from_fn::<_, 2, _>(|i| position[i] - cell[i]);
    Ok(std::array::from_fn(|i| {
        let x0 = a[i] + fraction[0] * (b[i] - a[i]);
        let x1 = c[i] + fraction[0] * (d[i] - c[i]);
        x0 + fraction[1] * (x1 - x0)
    }))
}

#[derive(Clone, Copy, Debug)]
pub struct Motion {
    pub low_velocity_scale: f32,
    pub attenuation_min: f32,
    pub attenuation_max: f32,
    pub wave_amount: f32,
    pub previous_velocity_multiplier: f32,
    pub wave_speed: f32,
}
impl Default for Motion {
    fn default() -> Self {
        // Effective parent instance overrides, not raw function defaults.
        Self {
            low_velocity_scale: 0.1,
            attenuation_min: 0.0,
            attenuation_max: 200.0,
            wave_amount: 1.0,
            previous_velocity_multiplier: 1.0,
            wave_speed: 1.0,
        }
    }
}

/// Native live Niagara grid branch (UseBakedSimHack=0). Incoming material WPO
/// and the advected wave-height function output are required upstream inputs;
/// no zero-wave fallback is supplied. Normal is the filtered RT sample and is
/// deliberately not renormalized before the native plane projection.
/// Evaluate once per frame phase, supplying that phase's upstream inputs:
/// native Time, transforms and incoming WPO are recompiled for previous frame.
pub fn grid_displacement(
    fluid: [f32; 4],
    normal: [f32; 3],
    advected_height: f32,
    incoming: [f32; 3],
    local_z_to_world: [f32; 3],
    engine_delta: f32,
    motion: Motion,
    previous_frame: bool,
) -> [f32; 3] {
    let height = advected_height + fluid[0];
    let current = local_z_to_world.map(|v| v * height);
    if !previous_frame {
        return std::array::from_fn(|i| incoming[i] + current[i]);
    }
    let magnitude = (fluid[2] * fluid[2] + fluid[3] * fluid[3]).sqrt();
    let t = ((magnitude - motion.attenuation_min)
        / (motion.attenuation_max - motion.attenuation_min))
        .clamp(0.0, 1.0);
    let smooth = t * t * (3.0 - 2.0 * t);
    let attenuation = (1.0 + (1.0 - motion.low_velocity_scale.clamp(0.0, 1.0)) * (smooth - 1.0))
        * motion.wave_amount;
    let velocity = [fluid[2], fluid[3], 0.0].map(|v| {
        (((v * engine_delta) * motion.previous_velocity_multiplier) * attenuation.clamp(0.0, 1.0))
            * motion.wave_speed
    });
    let dot = velocity[0] * normal[0] + velocity[1] * normal[1] + velocity[2] * normal[2];
    std::array::from_fn(|i| incoming[i] + (current[i] - (velocity[i] - dot * normal[i])))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn river_live_field_zero_load_edges_and_half_cell_coordinates() {
        let values = [[8.0, 4.0, 2.0, 1.0]; 6];
        assert_eq!(
            live_field([3, 2], &values, [0.0, 0.0]).unwrap(),
            [2.0, 1.0, 0.5, 0.25]
        );
        assert_eq!(live_field([3, 2], &values, [0.5, 0.5]).unwrap(), values[0]);
        assert_eq!(live_field([3, 2], &values, [-2.0, 1.0]).unwrap(), [0.0; 4]);
    }
    #[test]
    fn river_displacement_retains_nonunit_projection_and_incoming_wpo() {
        let motion = Motion {
            low_velocity_scale: 1.0,
            wave_speed: 2.0,
            ..Motion::default()
        };
        let evaluate = |height, incoming, previous| {
            grid_displacement(
                [100.0, 5.0, 40.0, 20.0],
                [0.5, 0.0, 0.5],
                height,
                incoming,
                [0.0, 1.0, 2.0],
                0.25,
                motion,
                previous,
            )
        };
        assert_eq!(evaluate(3.0, [1.0, 2.0, 3.0], false), [1.0, 105.0, 209.0]);
        assert_eq!(evaluate(3.0, [1.0, 2.0, 3.0], true), [-14.0, 95.0, 214.0]);
        assert_eq!(evaluate(4.0, [3.0, -1.0, 1.0], true), [-12.0, 93.0, 214.0]);
        assert_eq!(blur_normals([1, 1], &[[0.0; 4]]).unwrap(), [[0.0; 4]]);
        assert!(blur_normals([0, 1], &[]).is_err());
    }
}

//! Selected live MF_ShallowWaterRiver detail normals and opacity masks.
//! Original algorithms Copyright Epic Games, Inc. All Rights Reserved.
//! See surface-reference.json. Foam and scene inputs are explicit dependencies;
//! this is not the complete Single Layer Water material.

use super::{material::Motion, wave_advection::Height};

pub const SHADER: &str = include_str!("surface.wgsl");

/// Requires native material outputs, the shared FFT texture and real foam pins.
/// Includes fragment derivatives; use only from a fragment entry point.
pub fn fragment_shader() -> String {
    [
        super::material::SHADER,
        super::wave_advection::SHADER,
        SHADER,
    ]
    .join("\n")
}

/// The two advection calls use different scale/mip pins, even when their native
/// default scales coincide. WPO's camera fade does not attenuate its normal.
pub fn detail_advection() -> super::wave_advection::Settings {
    super::wave_advection::Settings {
        vertex_mip: 0.0,
        ..Default::default()
    }
}

/// MF_BlendNormals' basis rotation, including its south-pole singularity case.
/// Neither input nor output is normalized by this function in the source.
pub fn blend_normals(base: [f32; 3], detail: [f32; 3]) -> [f32; 3] {
    let a = 1.0 / (1.0 + base[2]);
    let b = -base[0] * base[1] * a;
    let (b1, b2) = if base[2] < -0.9999999 {
        ([0.0, -1.0, 0.0], [-1.0, 0.0, 0.0])
    } else {
        (
            [1.0 - base[0] * base[0] * a, b, -base[0]],
            [b, 1.0 - base[1] * base[1] * a, -base[1]],
        )
    };
    std::array::from_fn(|i| detail[0] * b1[i] + detail[1] * b2[i] + detail[2] * base[i])
}

fn smooth(min: f32, max: f32, value: f32) -> f32 {
    let t = ((value - min) / (max - min)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
fn attenuation(magnitude: f32, low: f32, min: f32, max: f32) -> f32 {
    1.0 + (smooth(min, max, magnitude) - 1.0) * (1.0 - low.clamp(0.0, 1.0))
}
fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// The SafeNormalize graph tests dot(v,v) against 1e-6 using a native If node.
/// Its connected equality pin and default 1e-5 tolerance matter at small lengths.
pub fn safe_normalize(normal: [f32; 3]) -> [f32; 3] {
    let squared: f32 = normal.iter().map(|v| v * v).sum();
    if (squared - 1e-6).abs() > 1e-5 && squared >= 1e-6 {
        normal.map(|v| v / squared.sqrt())
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Selected normal graph after sampling. WPO RGB and detail RGB are raw linear
/// FFT normals, without [0,1] remapping. Foam inputs are the upstream normalized
/// SW_FoamNormal and SW_TotalFoamAmount, not invented defaults.
pub fn normal(
    base: [f32; 3],
    wpo_rgb: [f32; 3],
    detail_rgb: [f32; 3],
    magnitude: f32,
    motion: Motion,
    height: Height,
    foam_normal: [f32; 3],
    total_foam: f32,
    foam_opacity: f32,
    opacity_mask: f32,
) -> [f32; 3] {
    let wpo_attenuation = attenuation(
        magnitude,
        height.low_velocity_scale,
        height.velocity_min,
        height.velocity_max,
    );
    let wpo_power = if wpo_attenuation > 2.980233e-8 {
        wpo_attenuation.powf(height.power)
    } else {
        0.0
    };
    let with_wpo = mix(
        base,
        blend_normals(base, wpo_rgb),
        (wpo_power * height.multiplier).clamp(0.0, 1.0),
    );
    let detail_attenuation = attenuation(
        magnitude,
        motion.low_velocity_scale,
        motion.attenuation_min,
        motion.attenuation_max,
    ) * motion.wave_amount;
    let with_detail = mix(
        with_wpo,
        blend_normals(with_wpo, detail_rgb),
        detail_attenuation.clamp(0.0, 1.0),
    );
    // Native foam basis uses with_wpo, before the separate detail-normal blend.
    let foam = blend_normals(with_wpo, foam_normal).map(|v| v * foam_opacity);
    let result = mix(with_detail, foam, total_foam);
    safe_normalize(if opacity_mask < 1e-8 {
        [0.0; 3]
    } else {
        result
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Masks {
    pub water_opacity: f32,
    pub simulation_mask: f32,
    pub material_mask: f32,
}

/// Selected live grid: no spline override; native one-texel border, >1e-8 depth
/// tests, and CustomDepth If-node equality band. Depth arguments are linearized
/// Unreal centimetres at the same pixel, never raw hardware depth.
pub fn masks(
    depth: f32,
    opacity: f32,
    uv: [f32; 2],
    dimensions: [u32; 2],
    incoming_mask: f32,
    pixel_depth: f32,
    custom_depth: f32,
) -> Masks {
    let water_opacity = if depth > 1e-8 { 1.0 } else { 0.0 } * opacity;
    let inside = (0..2).all(|i| {
        let dx = 1.0 / dimensions[i] as f32;
        uv[i] >= dx && uv[i] <= 1.0 - dx
    });
    let simulation_mask = if inside && water_opacity > 1e-8 {
        1.0
    } else {
        0.0
    };
    let custom_visible = if (pixel_depth - custom_depth).abs() > 1e-5 && pixel_depth < custom_depth
    {
        1.0
    } else {
        0.0
    };
    Masks {
        water_opacity,
        simulation_mask,
        material_mask: (incoming_mask * simulation_mask) * custom_visible,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn river_normal_basis_keeps_raw_magnitudes_and_south_pole() {
        assert_eq!(
            blend_normals([0.0, 0.0, 1.0], [0.3, -0.2, 0.8]),
            [0.3, -0.2, 0.8]
        );
        assert_eq!(
            blend_normals([0.0, 0.0, -1.0], [0.3, -0.2, 0.8]),
            [0.2, -0.3, -0.8]
        );
        assert_eq!(
            blend_normals([0.0, 0.0, 0.5], [0.0, 0.0, 0.8]),
            [0.0, 0.0, 0.4]
        );
    }

    #[test]
    fn river_safe_normalize_preserves_native_if_equality_band() {
        for x in [0.0, 0.001, 0.002, (1.099e-5f32).sqrt()] {
            assert_eq!(safe_normalize([x, 0.0, 0.0]), [0.0, 0.0, 1.0]);
        }
        assert_eq!(safe_normalize([0.004, 0.0, 0.0]), [1.0, 0.0, 0.0]);
    }

    #[test]
    fn river_foam_basis_precedes_separate_detail_normal() {
        let n = normal(
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            400.0,
            Motion::default(),
            Height::default(),
            [0.0, 0.0, 1.0],
            0.5,
            1.0,
            1.0,
        );
        for (a, b) in n.into_iter().zip([
            std::f32::consts::FRAC_1_SQRT_2,
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
        ]) {
            assert!((a - b).abs() < 1e-6);
        }
    }

    #[test]
    fn river_live_masks_keep_border_threshold_and_custom_depth_units() {
        let test = |depth, uv, pixel, custom| masks(depth, 0.7, uv, [8, 4], 0.25, pixel, custom);
        let inside = test(1.0, [0.125, 0.25], 100.0, 101.0);
        assert_eq!(
            inside,
            Masks {
                water_opacity: 0.7,
                simulation_mask: 1.0,
                material_mask: 0.25
            }
        );
        for uv in [[0.1249, 0.5], [0.8751, 0.5], [0.5, 0.2499], [0.5, 0.7501]] {
            assert_eq!(test(1.0, uv, 100.0, 101.0).simulation_mask, 0.0);
        }
        assert_eq!(test(1e-8, [0.5; 2], 0.0, 1.0).water_opacity, 0.0);
        for (pixel, custom) in [(1.0, 1.0), (1.0, 1.000005), (2.0, 1.0)] {
            assert_eq!(test(1.0, [0.5; 2], pixel, custom).material_mask, 0.0);
        }
        assert_eq!(
            masks(1.0, 1.0, [0.5; 2], [1, 8], 1.0, 0.0, 1.0).simulation_mask,
            0.0
        );
    }
}

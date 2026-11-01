//! Selected native primary-foam presentation graph (live non-VT path).
//! Original algorithms Copyright Epic Games, Inc. All Rights Reserved.
//! See foam-reference.json and foam-texture-reference.json. Hydraulic foam
//! generation and Single Layer Water lighting remain separate dependencies.
use super::wave_advection;

pub const SHADER: &str = include_str!("foam.wgsl");
pub fn fragment_shader() -> String {
    [super::surface::fragment_shader(), SHADER.to_owned()].join("\n")
}
pub fn advection() -> wave_advection::Settings {
    wave_advection::Settings {
        texture_scale: 0.001,
        vertex_mip: 0.0,
        ..Default::default()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub mask_fit: f32,
    pub mask_power: f32,
    pub scattering_fit: f32,
    pub scattering_power: f32,
    pub low_velocity_scale: f32,
    pub velocity_min: f32,
    pub velocity_max: f32,
    pub gain: f32,
    pub power: f32,
    pub black_point_to_mult: f32,
    pub render_fit: f32,
    pub normal_flatness: f32,
    pub opacity: f32,
    pub base_color: [f32; 3],
    pub roughness: f32,
    pub specular: f32,
    pub scattering_color: [f32; 4],
    pub scattering_velocity_min: f32,
    pub scattering_velocity_max: f32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            mask_fit: 1.0,
            mask_power: 1.0,
            scattering_fit: 1.0,
            scattering_power: 0.5,
            low_velocity_scale: 0.0,
            velocity_min: 25.0,
            velocity_max: 100.0,
            gain: 1.0,
            power: 1.0,
            black_point_to_mult: 1.0,
            render_fit: 0.25,
            normal_flatness: 0.3,
            opacity: 1.0,
            base_color: [0.95, 1.0, 0.9], // DinurdoJK user default; native reference remains documented
            roughness: 0.1,
            specular: 0.5,
            scattering_color: [0.427083, 1.0, 0.67074, 1.0],
            scattering_velocity_min: 100.0,
            scattering_velocity_max: 500.0,
        }
    }
}
fn smooth(min: f32, max: f32, x: f32) -> f32 {
    let t = ((x - min) / (max - min)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
fn power(base: f32, exponent: f32) -> f32 {
    if base > 2.980233e-8 {
        base.powf(exponent)
    } else {
        0.0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Foam {
    pub normal: [f32; 3],
    pub mask: f32,
    pub scattering_mask: f32,
    pub total: f32,
    pub opacity: f32,
    pub scattering: [f32; 3],
}

/// Selected SupportFoam=true, secondary=false, RGB-normal=true, debug=false.
/// `field` is the cubic reconstructed FoamRT R; `detail` is the advected native
/// HDR texture. Its raw RGB stays in the source's [0,1] range, without remapping.
pub fn present(field: f32, detail: [f32; 4], magnitude: f32, s: Settings) -> Foam {
    let low = 1.0
        + (smooth(s.velocity_min, s.velocity_max, magnitude) - 1.0)
            * (1.0 - s.low_velocity_scale.clamp(0.0, 1.0));
    let mask = (power((field * s.mask_fit).clamp(0.0, 1.0), s.mask_power) / s.mask_fit) * low;
    let scattering_mask = power(
        (field * s.scattering_fit).clamp(0.0, 1.0),
        s.scattering_power,
    ) / s.scattering_fit;
    let alpha = detail[3].clamp(0.0, 1.0);
    let black = smooth(1.0 - mask.clamp(0.0, 1.0), 1.0, alpha);
    let mult = mask * detail[3];
    let combined = black + (mult - black) * s.black_point_to_mult;
    let primary = power(combined, s.power) * s.gain;
    let total = smooth(0.0, s.render_fit, primary.max(0.0).clamp(0.0, 1.0));
    let n = std::array::from_fn::<_, 3, _>(|i| {
        detail[i] + ([0.0, 0.0, 1.0][i] - detail[i]) * s.normal_flatness
    });
    let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    let scattering_alpha = smooth(
        s.scattering_velocity_min,
        s.scattering_velocity_max,
        magnitude,
    )
    .max(scattering_mask);
    Foam {
        normal: n.map(|v| v / length),
        mask,
        scattering_mask,
        total,
        opacity: total * s.opacity,
        scattering: std::array::from_fn(|i| {
            (s.scattering_color[i] * s.scattering_color[3]) * scattering_alpha
        }),
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Attributes {
    pub base_color: [f32; 3],
    pub roughness: f32,
    pub specular: f32,
    pub opacity: f32,
}
/// Native SetMaterialAttributes adds foam opacity to incoming opacity, with no
/// extra saturate. Normal/WPO/mask use their separate graph functions.
pub fn attributes(incoming: Attributes, foam: Foam, s: Settings) -> Attributes {
    Attributes {
        base_color: std::array::from_fn(|i| {
            incoming.base_color[i] + (s.base_color[i] - incoming.base_color[i]) * foam.opacity
        }),
        roughness: incoming.roughness + (s.roughness - incoming.roughness) * foam.opacity,
        specular: incoming.specular + (s.specular - incoming.specular) * foam.opacity,
        opacity: incoming.opacity + foam.opacity,
    }
}

/// Native CubicInterpolateTexture (UseVT=false), including a hardware bilinear
/// clamped-sampler callback for its boundary branch. Interior loads need no OOB
/// policy because the native strict 1..size-3 cell range keeps all 16 taps inside.
pub fn cubic_field(
    dimensions: [u32; 2],
    uv: [f32; 2],
    mut load: impl FnMut([i32; 2]) -> [f32; 4],
    mut boundary: impl FnMut([f32; 2]) -> [f32; 4],
) -> [f32; 4] {
    let position = std::array::from_fn::<_, 2, _>(|i| uv[i] * dimensions[i] as f32 - 0.5);
    let cell = position.map(|v| v.floor() as i32);
    if (0..2).any(|i| cell[i] < 1 || cell[i] >= dimensions[i] as i32 - 2) {
        return boundary(uv);
    }
    let d = std::array::from_fn::<_, 2, _>(|i| position[i] - cell[i] as f32);
    let weights = |x: f32| {
        [
            -1.0 / 3.0 * x + 0.5 * x * x - 1.0 / 6.0 * x * x * x,
            1.0 - x * x + 0.5 * (x * x * x - x),
            x + 0.5 * (x * x - x * x * x),
            1.0 / 6.0 * (x * x * x - x),
        ]
    };
    let wx = weights(d[0]);
    let wy = weights(d[1]);
    let mut min = [f32::MAX; 4];
    let mut max = [-f32::MAX; 4];
    let mut out = [0.0; 4];
    for y in 0..4 {
        let mut row = [0.0; 4];
        for x in 0..4 {
            let value = load([cell[0] + x as i32 - 1, cell[1] + y as i32 - 1]);
            for c in 0..4 {
                row[c] += wx[x] * value[c];
                min[c] = min[c].min(value[c]);
                max[c] = max[c].max(value[c]);
            }
        }
        for c in 0..4 {
            out[c] += wy[y] * row[c];
        }
    }
    std::array::from_fn(|c| out[c].clamp(min[c], max[c]))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn river_foam_uses_raw_positive_rgb_and_velocity_gating() {
        let s = Settings::default();
        let dry = present(0.0, [0.5, 0.5, 1.0, 0.5], 200.0, s);
        assert_eq!(dry.total, 0.0);
        assert!(dry.normal[0] > 0.0 && dry.normal[1] > 0.0);
        assert_eq!(present(1.0, [0.5, 0.5, 1.0, 0.5], 25.0, s).total, 0.0);
        assert_eq!(present(1.0, [0.5, 0.5, 1.0, 0.5], 100.0, s).total, 1.0);
    }
    #[test]
    fn river_foam_scattering_is_not_low_speed_attenuated() {
        let f = present(1.0, [0.5, 0.5, 1.0, 0.5], 0.0, Settings::default());
        assert_eq!(f.mask, 0.0);
        assert_eq!(f.scattering_mask, 1.0);
        assert_eq!(f.scattering, [0.427083, 1.0, 0.67074]);
    }
    #[test]
    fn river_foam_does_not_clamp_added_material_opacity() {
        let s = Settings::default();
        let f = present(1.0, [0.5, 0.5, 1.0, 0.5], 200.0, s);
        let a = attributes(
            Attributes {
                base_color: [0.0; 3],
                roughness: 0.9,
                specular: 0.1,
                opacity: 0.75,
            },
            f,
            s,
        );
        assert_eq!(a.opacity, 1.75);
        assert_eq!(a.base_color, s.base_color);
    }
    #[test]
    fn river_foam_cubic_uses_native_small_texture_boundary_and_bounded_interiors() {
        assert_eq!(
            cubic_field(
                [2, 2],
                [0.5; 2],
                |_| panic!("native boundary must sample"),
                |_| [7.0; 4]
            ),
            [7.0; 4]
        );
        for uv in [[0.5; 2], [0.42, 0.61], [0.25, 0.75]] {
            let a = cubic_field(
                [8, 8],
                uv,
                |p| [if p == [3, 3] { 1.0 } else { 0.0 }; 4],
                |_| panic!("interior"),
            );
            assert!(a.iter().all(|v| (0.0..=1.0).contains(v)));
        }
    }
}

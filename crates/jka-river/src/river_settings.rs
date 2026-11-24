//! Authored material parameters. Defaults remain the selected Unreal instance.
use jka_water::river::{foam, slw};

pub const FLUID_NINJA_UNDERWATER: u8 = 2;
/// Default optics: Fluid Ninja when the private implementation is built in,
/// otherwise the shared Blue Water medium.
pub const DEFAULT_UNDERWATER_OPTICS: u8 = if jka_ninja::INCLUDED { FLUID_NINJA_UNDERWATER } else { 1 };

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub water: slw::Parameters,
    pub foam: foam::Settings,
    pub wave_amount: f32,
    pub wave_height: f32,
    /// Hybrid/Godot-style submerged optics; distances in JKA map units.
    pub underwater_color: [f32; 3],
    pub underwater_visibility: f32,
    pub underwater_darkening: f32,
    /// 0=original river visual (shared medium, no waterline tint),
    /// 1=shared Blue Water spectral medium + waterline tint, 2=Fluid Ninja.
    pub underwater_optics: u8,
    pub ninja_underwater: jka_ninja::NinjaUnderwaterSettings,
    /// Start of foam generation's depth gate, in map units. Full strength is
    /// derived using the native 150:500 ratio (60:200 map units).
    pub foam_minimum_depth: f32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            water: slw::Parameters::default(),
            foam: foam::Settings::default(),
            wave_amount: 1.0,
            wave_height: 5.0,
            underwater_color: jka_render_core::color::UNDERWATER_FOG_COLOR,
            underwater_visibility: 2304.0,
            underwater_darkening: 2304.0,
            underwater_optics: DEFAULT_UNDERWATER_OPTICS,
            ninja_underwater: jka_ninja::NinjaUnderwaterSettings::default(),
            foam_minimum_depth: 60.0,
        }
    }
}
impl Settings {
    /// RiverAuthored: SLW, foam, surface detail and submerged optics.
    pub fn uniform(self) -> [[f32; 4]; 27] {
        let p = self.water;
        let f = self.foam;
        let ninja = self.ninja_underwater;
        // Treat the shared River controls as edits relative to their reset
        // values. At reset the Ninja material keeps its authored coefficients,
        // texture contrast, and highlights.
        let water_defaults = slw::Parameters::default();
        let foam_defaults = foam::Settings::default();
        let scatter_scale = p.scattering[3] / water_defaults.scattering[3];
        let absorption_distance_scale = water_defaults.absorption[3] / p.absorption[3].max(1.0e-6);
        let scatter: [f32; 3] = std::array::from_fn(|i| {
            p.scattering[i] / water_defaults.scattering[i] * scatter_scale
        });
        let absorb: [f32; 3] = std::array::from_fn(|i| {
            water_defaults.absorption[i] / p.absorption[i].max(1.0e-6) * absorption_distance_scale
        });
        let albedo: [f32; 3] =
            std::array::from_fn(|i| p.water_albedo[i] / water_defaults.water_albedo[i]);
        let foam_color: [f32; 3] =
            std::array::from_fn(|i| f.base_color[i] / foam_defaults.base_color[i]);
        [
            p.scattering,
            p.absorption,
            [
                p.water_albedo[0],
                p.water_albedo[1],
                p.water_albedo[2],
                p.caustics_max_intensity,
            ],
            [
                p.water_specular,
                p.water_fresnel_specular,
                p.water_roughness,
                p.water_fresnel_roughness,
            ],
            [
                p.water_fresnel_exponent,
                p.anisotropy,
                p.velocity_to_mask_waves,
                0.0,
            ],
            [
                p.refraction,
                p.refraction_far,
                p.depth_for_min_refraction,
                p.refraction_bottom_amount,
            ],
            [
                p.critical_angle_dot,
                p.critical_angle_width,
                p.critical_angle_bend_width,
                p.lip_refraction,
            ],
            [p.refraction_depth_bias, 0.0, 0.0, 0.0],
            [
                f.mask_fit,
                f.mask_power,
                f.scattering_fit,
                f.scattering_power,
            ],
            [
                f.low_velocity_scale,
                f.velocity_min,
                f.velocity_max,
                f.normal_flatness,
            ],
            [f.gain, f.power, f.black_point_to_mult, f.render_fit],
            [f.opacity, f.roughness, f.specular, 0.0],
            [f.base_color[0], f.base_color[1], f.base_color[2], 0.0],
            f.scattering_color,
            [
                f.scattering_velocity_min,
                f.scattering_velocity_max,
                0.0,
                0.0,
            ],
            [self.wave_amount, self.wave_height, 0.0, 0.0],
            {
                let c = jka_render_core::color::linear_color(self.underwater_color);
                [c[0], c[1], c[2], self.underwater_visibility * 2.5]
            },
            [self.underwater_darkening * 2.5, f32::from(self.underwater_optics.min(FLUID_NINJA_UNDERWATER)), 0.0, 0.0],
            [ninja.distance_mult, ninja.distortion, ninja.brightness, ninja.caustics_mult],
            [ninja.distance_color[0], ninja.distance_color[1], ninja.distance_color[2], 0.0],
            [ninja.caustics_color[0], ninja.caustics_color[1], ninja.caustics_color[2], 0.0],
            [scatter[0], scatter[1], scatter[2], p.anisotropy - water_defaults.anisotropy],
            [absorb[0], absorb[1], absorb[2], 0.0],
            [
                albedo[0], albedo[1], albedo[2],
                (1.0 - f.normal_flatness) / (1.0 - foam_defaults.normal_flatness),
            ],
            [foam_color[0], foam_color[1], foam_color[2], f.gain],
            [
                p.water_roughness - water_defaults.water_roughness,
                p.water_fresnel_roughness - water_defaults.water_fresnel_roughness,
                p.water_specular / water_defaults.water_specular,
                p.water_fresnel_specular / water_defaults.water_fresnel_specular,
            ],
            [f.roughness - foam_defaults.roughness, f.specular / foam_defaults.specular, f.opacity, 0.0],
        ]
    }
}

#[cfg(test)]
mod underwater_optics_tests {
    use super::Settings;
    #[test]
    fn shared_defaults_on_and_keeps_existing_uniform_layout() {
        let mut shared = Settings::default();
        assert_eq!(shared.underwater_optics, super::DEFAULT_UNDERWATER_OPTICS);
        shared.underwater_optics = 1;
        assert_eq!(shared.uniform()[17][1], 1.0);
        let mut original = shared;
        original.underwater_optics = 0;
        assert_eq!(original.uniform()[17][1], 0.0);
        // Only the waterline tint changes: optical absorption and all
        // hydraulic/foam parameter slots are the same in both modes.
        let a = original.uniform();
        let b = shared.uniform();
        assert_eq!(&a[..17], &b[..17]);
        assert_eq!(a[17][0], b[17][0]);
        assert_eq!(a[17][2..], b[17][2..]);
        assert_eq!(&a[18..], &b[18..]);
    }
}

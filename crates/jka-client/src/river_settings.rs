//! Authored material parameters. Defaults remain the selected Unreal instance.
use jka_water::river::{foam, slw};

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
            underwater_color: crate::ocean::optics::Settings::default().fog_color,
            underwater_visibility: 2304.0,
            underwater_darkening: 2304.0,
            foam_minimum_depth: 60.0,
        }
    }
}
impl Settings {
    /// RiverAuthored: SLW, foam, surface detail and submerged optics.
    pub fn uniform(self) -> [[f32; 4]; 18] {
        let p = self.water;
        let f = self.foam;
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
                let c = crate::ocean::optics::linear_color(self.underwater_color);
                [c[0], c[1], c[2], self.underwater_visibility * 2.5]
            },
            [self.underwater_darkening * 2.5, 0.0, 0.0, 0.0],
        ]
    }
}

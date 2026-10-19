//! `SkyUniforms` in `shaders/common.wgsl`, field for field: every vec3 at a 16-byte offset followed by the
//! scalar packed with it. The test below checks the size against naga's layout of the WGSL struct.

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct SkyUniforms {
    /// Game units in the dome per unit of the tables' luminance: the sun (scale × lift × gain, 0 once set) and the
    /// moon (its colour over the sun's, × the grade's share), each times takram's sky radiance-to-luminance factors.
    pub sun_scale: [f32; 3],
    pub saturation: f32,
    pub moon_scale: [f32; 3],
    /// The authored gain on the higher scattering orders.
    pub multiple: f32,
    pub phase_lobe: [f32; 3],
    pub sun_lift: f32,
    pub phase_core: [f32; 3],
    /// Camera altitude (km) the twilight lift was taken for.
    pub camera_height: f32,
    /// Game units per unit of the tables for the light the clouds and the scene take: the sun's direct light and
    /// the sky's (with the dome's gain).
    pub direct_scale: [f32; 3],
    pub camera_horizon_cosine: f32,
    pub diffuse_scale: [f32; 3],
    /// Game units per unit of relative luminance (`model::game_scale`).
    pub game_scale: f32,
    pub sun_direction: [f32; 3],
    pub sun_active: f32,
    pub sun_irradiance: [f32; 3],
    pub moon_active: f32,
    pub moon_direction: [f32; 3],
    pub moon_phase: f32,
    pub moon_irradiance: [f32; 3],
    pub time: f32,
    pub camera_position: [f32; 3],
    /// The sun's elevation (degrees) above the camera's horizon, as the twilight lift reads it.
    pub sun_elevation: f32,
    pub galactic_x: [f32; 3],
    pub night: f32,
    pub galactic_y: [f32; 3],
    pub star_horizon: f32,
    pub galactic_z: [f32; 3],
    pub star_limit: f32,
    pub moon_right: [f32; 3],
    pub star_gain: f32,
    pub moon_up: [f32; 3],
    pub milky_way: f32,
    pub moon_sun: [f32; 3],
    pub moon_lit: f32,
    pub moon_surface: [f32; 3],
    pub moon_opposition: f32,
    pub earthshine: [f32; 3],
    pub moon_glow: f32,
    pub forward: [f32; 3],
    pub pixel_angle: f32,
    pub light_direction: [f32; 3],
    pub bake_spread: f32,
    pub light_color: [f32; 3],
    pub bake_level: f32,
    pub inverse_projection: [[f32; 4]; 4],
    pub view_to_world: [[f32; 4]; 4],
    pub viewport: [f32; 4],
    /// Accumulated cloud drift (m) in world XZ (`SkyUniforms.windOffset`); y is 0.
    pub wind_offset: [f32; 3],
    /// Lightning's scene flash, a multiple of the ambient (`SkyUniforms.flash`).
    pub flash: f32,
    /// The lightning channel lighting the clouds (world m) and its radiant intensity (0 dark).
    pub lightning_position: [f32; 3],
    pub lightning_intensity: f32,
    /// How the air under a closed deck greys (x, 0–1) and dims (y) the clear sky's haze (`overcast`); z is the dome's night sky brightness (1 authored, `Sky::set_night_sky_brightness`); w unused.
    pub haze: [f32; 4],
}

pub(crate) fn v3(v: glam::DVec3) -> [f32; 3] {
    v.as_vec3().to_array()
}

#[cfg(test)]
mod tests {
    use super::SkyUniforms;

    /// The WGSL struct's size, as naga lays it out, equals the Rust struct's.
    #[test]
    fn rust_and_wgsl_layouts_agree() {
        let module = crate::shaders::compose(crate::shaders::DOME_WGSL, "dome.wgsl").unwrap();
        let span = module
            .types
            .iter()
            .find_map(|(_, ty)| match (&ty.name, &ty.inner) {
                (Some(name), wgpu::naga::TypeInner::Struct { members, span })
                    if name.starts_with("SkyUniforms") =>
                {
                    let names: Vec<_> = members.iter().filter_map(|m| m.name.clone()).collect();
                    assert_eq!(names.first().map(String::as_str), Some("sun_scale"));
                    assert_eq!(names.last().map(String::as_str), Some("haze"));
                    let offset_of = |field: &str| {
                        members
                            .iter()
                            .find(|m| m.name.as_deref() == Some(field))
                            .map(|m| m.offset)
                    };
                    assert_eq!(
                        offset_of("inverse_projection"),
                        Some(std::mem::offset_of!(SkyUniforms, inverse_projection) as u32)
                    );
                    assert_eq!(
                        offset_of("galactic_x"),
                        Some(std::mem::offset_of!(SkyUniforms, galactic_x) as u32)
                    );
                    assert_eq!(
                        offset_of("diffuse_scale"),
                        Some(std::mem::offset_of!(SkyUniforms, diffuse_scale) as u32)
                    );
                    assert_eq!(
                        offset_of("lightning_position"),
                        Some(std::mem::offset_of!(SkyUniforms, lightning_position) as u32)
                    );
                    Some(*span)
                }
                _ => None,
            })
            .expect("SkyUniforms in the dome's module");
        assert_eq!(span as usize, std::mem::size_of::<SkyUniforms>());
        assert_eq!(std::mem::size_of::<SkyUniforms>(), 544);
    }
}

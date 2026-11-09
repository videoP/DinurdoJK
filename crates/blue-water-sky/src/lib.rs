//! Original Blue Water atmosphere and celestial renderer, on JKA host services.
pub use glam as math;
mod sky_mean;
pub use sky_mean::SkyMeanSampler;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/aerial.rs"]
pub mod aerial;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/cover.rs"]
mod cover;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/environment.rs"]
pub mod environment;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/gpu.rs"]
mod gpu;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/half.rs"]
mod half;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/haze.rs"]
pub mod haze;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/model.rs"]
pub mod model;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/shaders.rs"]
pub mod shaders;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/sky.rs"]
mod sky;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/stars.rs"]
pub mod stars;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/state.rs"]
pub mod state;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/tables.rs"]
mod tables;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/uniforms.rs"]
mod uniforms;
pub use cover::{CloudCover, CloudCoverFrame};
pub use environment::*;
pub use shaders::add_sky_modules;
pub use sky::{CloudHooks, SeaLevel, Sky, SkyPipelines, SkyQuality, SkyTargets, SkyView};
pub use state::{
    CloudGenus, CloudScene, SkyAngles, DEFAULT_SKY_SCENE, HighCloudGenus, HighClouds, SkyScene, WeatherScene,
};
pub use uniforms::SkyUniforms;

/// Shader definition for host scene targets; the clear-sky BSP shader uses
/// the same upstream atmosphere modules without requiring naga_oil in the host.
pub mod host_shader_defs {
    pub fn sdr_scene(format: wgpu::TextureFormat) -> naga_oil::compose::ShaderDefValue {
        naga_oil::compose::ShaderDefValue::Bool(format != wgpu::TextureFormat::Rgba16Float)
    }
}

#[cfg(test)]
#[path = "../../jka-client/src/renderer/lighting/baked_sun_shader.rs"]
mod lightmap_composition_shader;

#[cfg(test)]
mod host_shader_tests {
    #[test]
    fn baked_cubemap_shaders_validate_for_sdr_and_fp16() {
        let cube = crate::shaders::compose(
            include_str!("../../jka-client/src/renderer/environment/blue_water_sky_cube.wgsl"),
            "jka/blue_water_sky_cube.wgsl",
        ).expect("compose baked sky cube");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        ).validate(&cube).expect("validate baked sky cube");
        for format in [wgpu::TextureFormat::Bgra8UnormSrgb, wgpu::TextureFormat::Rgba16Float] {
            let display = crate::shaders::compose_with(
                include_str!("../../jka-client/src/renderer/environment/blue_water_sky_cached.wgsl"),
                "jka/blue_water_sky_cached.wgsl", &[],
                &[("SKY_SDR", crate::host_shader_defs::sdr_scene(format))],
            ).expect("compose cached sky display");
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::all(),
            ).validate(&display).expect("validate cached sky display");
        }
    }

    #[test]
    fn generated_lightmap_composition_shader_validates() {
        let source = crate::lightmap_composition_shader::cache_shader();
        let module = crate::shaders::compose(&source, "JKA lightmap composition")
            .expect("compose the actual runtime lightmap shader");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        ).validate(&module).expect("validate the actual runtime lightmap shader");
    }

    #[test]
    fn direct_sky_validates_and_maps_only_sdr() {
        let mut divisions = Vec::new();
        for format in [wgpu::TextureFormat::Bgra8UnormSrgb, wgpu::TextureFormat::Rgba16Float] {
            let module = crate::shaders::compose_with(
                include_str!("../../jka-client/src/renderer/environment/blue_water_sky_direct.wgsl"),
                "jka/blue_water_sky_direct.wgsl",
                &[],
                &[("SKY_SDR", crate::host_shader_defs::sdr_scene(format))],
            ).expect("compose direct BSP sky");
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::default(),
            ).validate(&module).expect("validate direct BSP sky");
            let fragment = module.entry_points.iter().find(|entry| entry.name == "sky_color").unwrap();
            divisions.push(fragment.function.expressions.iter().filter(|(_, expr)| matches!(expr,
                wgpu::naga::Expression::Binary { op: wgpu::naga::BinaryOperator::Divide, .. }
            )).count());
        }
        assert_eq!(divisions[0], divisions[1] + 1, "only SDR divides radiance by 1 + radiance");
    }

    #[test]
    fn isolated_vertex_sky_shader_validates() {
        let source = include_str!("../../jka-client/src/renderer/lighting/vertex_sky.wgsl");
        let module = crate::shaders::compose(source, "jka/vertex_sky.wgsl").expect("compose isolated vertex sky");
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::default(),
        ).validate(&module).expect("validate isolated vertex sky");
    }
}

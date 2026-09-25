pub(crate) mod fog;
mod rain;

pub(crate) use fog::{FogSystem, FroxelUniform, FROXEL_X, FROXEL_Y, FROXEL_Z};
pub(crate) use rain::{
    rain_haze_mask_dimensions, RainSystem, WeatherOcclusionCache, WeatherSurfaceUniform,
    RAIN_PUDDLE_EPSILON, RAIN_WETNESS_EPSILON, RAIN_WETNESS_FADE_END, RAIN_WETNESS_FADE_START,
};

pub(crate) struct WeatherSystem {
    pub rain: RainSystem,
    pub fog: FogSystem,
}

impl WeatherSystem {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera_layout: &wgpu::BindGroupLayout,
        surface_format: wgpu::TextureFormat,
        sample_count: u32,
        wind_noise_texture: &wgpu::Texture,
    ) -> Self {
        Self {
            rain: RainSystem::new(
                device,
                queue,
                camera_layout,
                surface_format,
                sample_count,
                wind_noise_texture,
            ),
            fog: FogSystem::new(device),
        }
    }
}

mod cpu_field;
mod field;
pub(crate) mod fog;
mod rain;
pub(crate) mod wake;
#[cfg(test)]
pub(crate) use rain::layout_tests as rain_layout;

pub(crate) use fog::{FogSystem, FroxelUniform, FROXEL_X, FROXEL_Y, FROXEL_Z};
pub(crate) use rain::{
    rain_haze_mask_dimensions, RainSystem, WeatherOcclusionCache, WeatherSurfaceEnvironment,
    WeatherSurfaceUniform,
    RAIN_PUDDLE_EPSILON, RAIN_WETNESS_EPSILON, RAIN_WETNESS_FADE_END, RAIN_WETNESS_FADE_START,
};

const WEATHER_SURFACE_WGSL: &str = include_str!("weather_surface.wgsl");
const WEATHER_SURFACE_WORLD_WGSL: &str = include_str!("weather_surface_world.wgsl");
const WEATHER_WAKE_WGSL: &str = include_str!("weather_wake.wgsl");

const POST_SETTINGS_WGSL: &str = include_str!("post_settings.wgsl");

/// `base` plus the post-process uniform block, for shaders that bind the post buffer.
pub(crate) fn with_post_settings(base: &str) -> String {
    format!("{POST_SETTINGS_WGSL}
{base}")
}

/// `base` plus the shared weather maths (puddle shape, ripples, water BRDF) for
/// shaders that keep their own bindings: the post pass, SSR trace and rain splashes.
pub(crate) fn with_weather_surface(base: &str) -> String {
    format!("{base}
{WEATHER_SURFACE_WGSL}")
}

/// [`with_weather_surface`] for shaders that also declare the `weather_surface`
/// uniform (the SSR trace), which adds the footstep wake ripples.
pub(crate) fn with_bound_weather_surface(base: &str) -> String {
    format!("{base}
{WEATHER_SURFACE_WGSL}
{WEATHER_WAKE_WGSL}")
}

/// `base` plus the shared weather maths and forward weather shading for the world
/// shaders, which share one set of module-scope bindings.
pub(crate) fn with_world_weather(base: &str) -> String {
    format!("{base}
{WEATHER_SURFACE_WGSL}
{WEATHER_WAKE_WGSL}
{WEATHER_SURFACE_WORLD_WGSL}")
}

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

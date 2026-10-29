mod atmosphere;
mod cpu_field;
mod field;
pub(crate) mod fog;
mod rain;
mod rain_glass;
mod snow;
mod wind_visuals;
pub(crate) mod wake;
#[cfg(test)]
pub(crate) use rain::layout_tests as rain_layout;

pub(crate) use atmosphere::AtmosphereSystem;
pub(crate) use fog::{FogSystem, FroxelUniform, FROXEL_X, FROXEL_Y, FROXEL_Z};
pub(crate) use rain::{
    rain_haze_mask_dimensions, RainSystem, WeatherOcclusionCache, WeatherSurfaceEnvironment,
    WeatherSurfaceUniform,
    RAIN_PUDDLE_EPSILON, RAIN_SKY_BLEND_CEILING, RAIN_SKY_BLEND_WINDOW, RAIN_WETNESS_EPSILON,
    RAIN_WETNESS_FADE_END, RAIN_WETNESS_FADE_START,
};
pub(crate) use rain_glass::RainGlassResources;
pub(crate) use snow::SnowSystem;
pub(crate) use wind_visuals::WindVisualSystem;

const WEATHER_SURFACE_WGSL: &str = include_str!("weather_surface.wgsl");
const WEATHER_SURFACE_WORLD_WGSL: &str = include_str!("weather_surface_world.wgsl");
const WEATHER_WAKE_WGSL: &str = include_str!("weather_wake.wgsl");

const MODEL_WEATHER_WGSL: &str = include_str!("model_weather.wgsl");

const POST_SETTINGS_WGSL: &str = include_str!("post_settings.wgsl");

/// `base` plus the post-process uniform block, for shaders that bind the post buffer.
pub(crate) fn with_post_settings(base: &str) -> String {
    // Reuse the upstream display math without its standalone capture bindings.
    const DISPLAY_MARKER: &str = "// BLUE_WATER_DISPLAY_LIBRARY";
    let base = if base.contains(DISPLAY_MARKER) {
        let upstream = include_str!("../../../vendor/blue-water-sea-and-sky/sky/shaders/display.wgsl");
        let library = upstream.split_once("const LOOK_EXPOSURE").unwrap().1;
        let library = format!("const LOOK_EXPOSURE{}", library.split_once("@vertex").unwrap().0)
            .replace("luminance(", "bw_display_luminance(")
            .replace("display_grade(", "bw_display_grade(")
            .replace("agx_contrast(", "bw_agx_contrast(")
            .replace("agx(", "bw_agx(");
        base.replace(DISPLAY_MARKER, &library)
    } else {
        base.to_owned()
    };
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

/// `base` plus the shared weather maths and the entity wetness shading, for the
/// model shaders (`md3`, `ghoul2_skin`). They carry no weather bindings of their
/// own: the values ride in the entity-lighting uniform they already bind.
pub(crate) fn with_model_weather(base: &str) -> String {
    format!("{base}
{}
{WEATHER_SURFACE_WGSL}
{MODEL_WEATHER_WGSL}
{}", crate::foliage::SHADER, include_str!("legacy_model_blend.wgsl"))
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
    pub snow: SnowSystem,
    pub atmosphere: AtmosphereSystem,
    pub fog: FogSystem,
    pub wind_visuals: WindVisualSystem,
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
        let rain = RainSystem::new(
            device,
            queue,
            camera_layout,
            surface_format,
            sample_count,
            wind_noise_texture,
        );
        let snow = SnowSystem::new(
            device,
            camera_layout,
            surface_format,
            sample_count,
            rain.collision_view(),
            &rain.gpu.wind_noise_view,
            &rain.gpu.wind_noise_sampler,
        );
        let wind_visuals = WindVisualSystem::new(
            device,
            camera_layout,
            surface_format,
            sample_count,
            rain.collision_view(),
            &rain.gpu.wind_noise_view,
            &rain.gpu.wind_noise_sampler,
        );
        Self {
            rain,
            snow,
            atmosphere: AtmosphereSystem::new(
                device,
                camera_layout,
                surface_format,
                sample_count,
            ),
            fog: FogSystem::new(device),
            wind_visuals,
        }
    }
}

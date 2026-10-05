//! Environment fog.
use crate::renderer::{
    bevy_cascade_bounds, weather, DynamicShadowsMode, FroxelUniform, Mat4, Renderer, Vec3,
    WeatherOcclusionCache, SHADOW_CASCADES, SHADOW_MAP_SIZE,
};

impl Renderer {
    pub(in crate::renderer) fn write_froxel_uniform(
        &self,
        inv_view_proj: Mat4,
        camera_pos_time: [f32; 4],
        shadow_matrices: Option<&[Mat4; SHADOW_CASCADES]>,
    ) {
        let matrices = shadow_matrices
            .copied()
            .unwrap_or([Mat4::IDENTITY; SHADOW_CASCADES]);
        let sun = self.active_sun();
        let sun_direction = Vec3::from_array(sun.direction).normalize_or_zero();
        let (fog_density, fog_color, fog_height_falloff, map_fog_depth) =
            self.weather.fog.volumetric_density_color();
        let rain_occlusion = match &self.weather.rain.occlusion {
            WeatherOcclusionCache::Ready(info) => [
                info.min_xz[0],
                info.min_xz[1],
                info.inv_extent_xz[0],
                info.inv_extent_xz[1],
            ],
            WeatherOcclusionCache::Pending(_)
            | WeatherOcclusionCache::Building(_)
            | WeatherOcclusionCache::Unavailable => [0.0; 4],
        };
        let weather_wind = self.weather_wind.sanitize();
        let weather_wind_angle = weather_wind.direction.to_radians();
        let uniform = FroxelUniform {
            inv_view_proj: inv_view_proj.to_cols_array_2d(),
            camera_pos_time,
            sun_direction_enabled: [sun_direction.x, sun_direction.y, sun_direction.z, 1.0],
            sun_color_intensity: [sun.color[0], sun.color[1], sun.color[2], sun.intensity],
            // w is zero for manual fog and depthForOpaque for authored MAP
            // fog. With authored fog, x is a multiplier of the OpenJK EXP2
            // profile (MAP/0 and explicit 1.0x are identical). With manual fog
            // x remains the Beer-Lambert base density.
            fog_params: [fog_density, fog_height_falloff, 768.0, map_fog_depth],
            fog_color_anisotropy: [fog_color[0], fog_color[1], fog_color[2], 0.35],
            rain_params: [
                if self.weather.rain.enabled { 1.0 } else { 0.0 },
                self.weather.rain.intensity.shader_value(),
                self.weather.rain.intensity.haze_strength(),
                0.0,
            ],
            rain_occlusion,
            rain_wind: [
                weather_wind_angle.cos() * weather_wind.speed,
                weather_wind_angle.sin() * weather_wind.speed,
                0.0,
                0.0,
            ],
            shadow_view_proj: std::array::from_fn(|index| matrices[index].to_cols_array_2d()),
            split_depths: bevy_cascade_bounds(),
            shadow_params: [
                SHADOW_MAP_SIZE as f32,
                0.0006,
                if self.cascaded_shadows_enabled && shadow_matrices.is_some() {
                    1.0
                } else {
                    0.0
                },
                if self.cascaded_shadow_mode == DynamicShadowsMode::CascadedShadowMaps {
                    1.0
                } else {
                    0.0
                },
            ],
            grid: [
                weather::FROXEL_X,
                weather::FROXEL_Y,
                weather::FROXEL_Z,
                self.sun_visibility.shader_value() as u32,
            ],
        };
        self.queue.write_buffer(
            &self.weather.fog.resources.uniform_buffer,
            0,
            bytemuck::bytes_of(&uniform),
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::renderer) fn create_world_froxel_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniform_buffer: &wgpu::Buffer,
    fog_buffer: &wgpu::Buffer,
    sun_shadow_array: &wgpu::TextureView,
    sky_admission_array: &wgpu::TextureView,
    light_buffer: &wgpu::Buffer,
    cluster_buffer: &wgpu::Buffer,
    lighting_settings_buffer: &wgpu::Buffer,
    local_shadow_cube: &wgpu::TextureView,
    local_shadow_sampler: &wgpu::Sampler,
    rain_occlusion_height: &wgpu::TextureView,
    wind_noise: &wgpu::TextureView,
    wind_noise_sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA world froxel volumetric fog bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: fog_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(sun_shadow_array),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: light_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: cluster_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: lighting_settings_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(local_shadow_cube),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::Sampler(local_shadow_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(rain_occlusion_height),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: wgpu::BindingResource::TextureView(wind_noise),
            },
            wgpu::BindGroupEntry {
                binding: 10,
                resource: wgpu::BindingResource::Sampler(wind_noise_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: wgpu::BindingResource::TextureView(sky_admission_array),
            },
        ],
    })
}

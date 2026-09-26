use crate::{
    camera::Camera,
    scene::WeatherOcclusionSource,
    ui::RainIntensity,
};
use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use std::{
    sync::Arc,
    thread::{self, JoinHandle},
    time::Instant,
};
use wgpu::util::DeviceExt;

pub(crate) const RAIN_MAX_PARTICLES: u32 = 32_768;
pub(crate) const RAIN_COMPUTE_GROUP_SIZE: u32 = 128;
const WEATHER_OCCLUSION_TARGET_TEXEL: f32 = 8.0;
const WEATHER_OCCLUSION_MIN_AXIS: u32 = 64;
const WEATHER_OCCLUSION_MAX_AXIS: u32 = 1024;
pub(crate) const RAIN_HAZE_MASK_WIDTH: u32 = 64;
pub(crate) const RAIN_HAZE_MASK_MIN_HEIGHT: u32 = 16;
pub(crate) const RAIN_HAZE_MASK_MAX_HEIGHT: u32 = 64;
pub(crate) const WEATHER_NO_SURFACE_HEIGHT: f32 = -1.0e20;
pub(crate) const RAIN_WETNESS_FADE_START: f32 = 450.0;
pub(crate) const RAIN_WETNESS_FADE_END: f32 = 1150.0;
pub(crate) const RAIN_WETNESS_RISE_SECONDS: f32 = 2.5;
pub(crate) const RAIN_WETNESS_DRY_SECONDS: f32 = 12.0;
pub(crate) const RAIN_WETNESS_EPSILON: f32 = 0.002;
pub(crate) const RAIN_PUDDLE_RISE_SECONDS: f32 = 18.0;
pub(crate) const RAIN_PUDDLE_DRY_SECONDS: f32 = 42.0;
pub(crate) const RAIN_PUDDLE_EPSILON: f32 = 0.002;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

fn camera_depth_compare() -> wgpu::CompareFunction {
    wgpu::CompareFunction::GreaterEqual
}

fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding, visibility,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}

fn storage_buffer_entry(binding: u32, read_only: bool, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding, visibility,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}

fn unfilterable_texture_entry_compute(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding, visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2, multisampled: false,
        }, count: None,
    }
}

fn filterable_texture_entry_compute(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding, visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2, multisampled: false,
        }, count: None,
    }
}

fn filtering_sampler_entry_compute(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding, visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct WeatherSurfaceUniform {
    pub amount_distance: [f32; 4],
    pub puddle: [f32; 4],
    pub occlusion_uv: [f32; 4],
    pub occlusion_size: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RainParticle { position_state: [f32; 4], velocity_age: [f32; 4], misc: [f32; 4] }
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RainSimUniform {
    camera_dt: [f32; 4],
    spawn: [f32; 4],
    wind_time: [f32; 4],
    weather: [f32; 4],
    collision_uv: [f32; 4],
    counts: [u32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RainRenderUniform { appearance: [f32; 4], splash: [f32; 4], color: [f32; 4] }

#[derive(Clone, Copy)]
pub(crate) struct WeatherOcclusionInfo { pub min_xz: [f32; 2], pub inv_extent_xz: [f32; 2], pub width: u32, pub height: u32 }

pub(crate) struct WeatherOcclusionBuildResult {
    pub info: WeatherOcclusionInfo, pub surface_field: Vec<[f32; 4]>,
    pub brush_count: usize, pub triangle_count: usize, pub topography_triangle_count: usize, pub build_ms: f64,
}

pub(crate) enum WeatherOcclusionCache {
    Unavailable, Pending(Arc<WeatherOcclusionSource>), Building(JoinHandle<Option<WeatherOcclusionBuildResult>>), Ready(WeatherOcclusionInfo),
}

pub(crate) struct RainGpuResources {
    pub _collision_texture: wgpu::Texture,
    pub collision_view: wgpu::TextureView,
    // A view of GrassRenderer's canonical GodotGrass wind texture plus a cheap
    // repeat/linear sampler. The underlying texture is owned only once by grass.
    pub wind_noise_view: wgpu::TextureView,
    pub wind_noise_sampler: wgpu::Sampler,
    pub particle_buffer: wgpu::Buffer,
    pub sim_uniform_buffer: wgpu::Buffer,
    pub sim_layout: wgpu::BindGroupLayout,
    pub sim_bind_group: wgpu::BindGroup,
    pub sim_pipeline: wgpu::ComputePipeline,
    pub render_uniform_buffer: wgpu::Buffer,
    pub render_bind_group: wgpu::BindGroup,
    pub render_pipeline_layout: wgpu::PipelineLayout,
    pub render_shader: wgpu::ShaderModule,
    pub drop_pipeline: wgpu::RenderPipeline,
    pub splash_pipeline: wgpu::RenderPipeline,
}

pub(crate) struct RainHazeResources {
    pub layout: wgpu::BindGroupLayout,
    pub bind_group: Option<wgpu::BindGroup>,
    pub pipeline: wgpu::ComputePipeline,
}

pub(crate) struct RainSystem {
    pub gpu: RainGpuResources,
    pub occlusion: WeatherOcclusionCache,
    pub haze: RainHazeResources,
    pub enabled: bool,
    pub intensity: RainIntensity,
    pub surface_wetness: f32,
    pub puddle_amount: f32,
    pub puddle_debug_visualization: bool,
    pub surface_wetness_last_time: f32,
}

impl RainSystem {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera_layout: &wgpu::BindGroupLayout,
        surface_format: wgpu::TextureFormat,
        sample_count: u32,
        wind_noise_texture: &wgpu::Texture,
    ) -> Self {
        Self {
            gpu: create_rain_gpu_resources(
                device,
                queue,
                camera_layout,
                surface_format,
                sample_count,
                wind_noise_texture,
            ),
            occlusion: WeatherOcclusionCache::Unavailable,
            haze: create_rain_haze_resources(device),
            enabled: false,
            intensity: RainIntensity::Rain,
            surface_wetness: 0.0,
            puddle_amount: 0.0,
            puddle_debug_visualization: false,
            surface_wetness_last_time: 0.0,
        }
    }

    pub(crate) fn start_map_occlusion(&mut self, source: Option<WeatherOcclusionSource>) {
        self.occlusion = source
            .map(start_weather_occlusion_build)
            .unwrap_or(WeatherOcclusionCache::Unavailable);
        self.surface_wetness = 0.0;
        self.puddle_amount = 0.0;
        self.surface_wetness_last_time = 0.0;
    }

    pub(crate) fn rebuild_haze_bind_group(
        &mut self,
        device: &wgpu::Device,
        post_buffer: &wgpu::Buffer,
        linear_depth_view: &wgpu::TextureView,
        haze_mask_view: &wgpu::TextureView,
    ) {
        self.haze.bind_group = Some(create_rain_haze_mask_bind_group(
            device,
            &self.haze.layout,
            post_buffer,
            linear_depth_view,
            &self.gpu.collision_view,
            haze_mask_view,
            &self.gpu.wind_noise_view,
            &self.gpu.wind_noise_sampler,
        ));
    }

    pub(crate) fn rebuild_render_pipelines(
        &mut self,
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        sample_count: u32,
    ) {
        self.gpu.drop_pipeline = create_rain_drop_pipeline(
            device,
            &self.gpu.render_pipeline_layout,
            &self.gpu.render_shader,
            surface_format,
            sample_count,
        );
        self.gpu.splash_pipeline = create_rain_splash_pipeline(
            device,
            &self.gpu.render_pipeline_layout,
            &self.gpu.render_shader,
            surface_format,
            sample_count,
        );
    }

    pub(crate) fn collision_view(&self) -> &wgpu::TextureView {
        &self.gpu.collision_view
    }

    pub(crate) fn dispatch_simulation(&self, encoder: &mut wgpu::CommandEncoder) {
        if !self.enabled {
            return;
        }
        let count = self.intensity.particle_count().min(RAIN_MAX_PARTICLES);
        let groups = (count + RAIN_COMPUTE_GROUP_SIZE - 1) / RAIN_COMPUTE_GROUP_SIZE;
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("JKA GPU rain simulation"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.gpu.sim_pipeline);
        pass.set_bind_group(0, &self.gpu.sim_bind_group, &[]);
        pass.dispatch_workgroups(groups, 1, 1);
    }

    pub(crate) fn render<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        camera_bind_group: &'a wgpu::BindGroup,
    ) {
        if !self.enabled {
            return;
        }
        let count = self.intensity.particle_count().min(RAIN_MAX_PARTICLES);
        pass.set_pipeline(&self.gpu.drop_pipeline);
        pass.set_bind_group(0, camera_bind_group, &[]);
        pass.set_bind_group(1, &self.gpu.render_bind_group, &[]);
        pass.draw(0..4, 0..count);

        pass.set_pipeline(&self.gpu.splash_pipeline);
        let splash_count = self.intensity.splash_particle_count().min(count);
        pass.draw(0..6, 0..splash_count);
    }

    pub(crate) fn dispatch_haze(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        width: u32,
        height: u32,
    ) {
        let Some(bind_group) = self.haze.bind_group.as_ref() else {
            return;
        };
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("JKA rain haze exposure"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.haze.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
    }

    pub(crate) fn advance_surface_wetness(&mut self, frame_time: f32) -> bool {
        let was_active = self.surface_wetness > RAIN_WETNESS_EPSILON
            || self.puddle_amount > RAIN_PUDDLE_EPSILON;
        let dt = if self.surface_wetness_last_time > 0.0 {
            (frame_time - self.surface_wetness_last_time).clamp(0.0, 0.25)
        } else {
            1.0 / 120.0
        };
        self.surface_wetness_last_time = frame_time;

        let wet_target = if self.enabled { 1.0 } else { 0.0 };
        if wet_target > self.surface_wetness {
            self.surface_wetness = (self.surface_wetness + dt / RAIN_WETNESS_RISE_SECONDS).min(wet_target);
        } else {
            self.surface_wetness = (self.surface_wetness - dt / RAIN_WETNESS_DRY_SECONDS).max(wet_target);
        }

        let puddle_target = if self.enabled {
            match self.intensity {
                RainIntensity::Light => 0.22,
                RainIntensity::Rain => 0.62,
                RainIntensity::Heavy => 1.0,
            }
        } else {
            0.0
        };
        if puddle_target > self.puddle_amount {
            self.puddle_amount = (self.puddle_amount + dt / RAIN_PUDDLE_RISE_SECONDS).min(puddle_target);
        } else {
            self.puddle_amount = (self.puddle_amount - dt / RAIN_PUDDLE_DRY_SECONDS).max(puddle_target);
        }

        if !self.enabled
            && self.surface_wetness <= RAIN_WETNESS_EPSILON
            && self.puddle_amount <= RAIN_PUDDLE_EPSILON
        {
            self.surface_wetness = 0.0;
            self.puddle_amount = 0.0;
            self.surface_wetness_last_time = 0.0;
        }
        was_active && self.surface_wetness == 0.0 && self.puddle_amount == 0.0
    }

    pub(crate) fn write_surface_uniform(&self, queue: &wgpu::Queue, buffer: &wgpu::Buffer) {
        let (occlusion_uv, width, height, active) = match &self.occlusion {
            WeatherOcclusionCache::Ready(info) => (
                [info.min_xz[0], info.min_xz[1], info.inv_extent_xz[0], info.inv_extent_xz[1]],
                info.width, info.height, 1,
            ),
            WeatherOcclusionCache::Pending(_) | WeatherOcclusionCache::Building(_) | WeatherOcclusionCache::Unavailable => ([0.0; 4], 0, 0, 0),
        };
        let intensity = match self.intensity {
            RainIntensity::Light => 0.72,
            RainIntensity::Rain => 0.88,
            RainIntensity::Heavy => 1.0,
        };
        let ripple_intensity = match self.intensity {
            RainIntensity::Light => 0.28,
            RainIntensity::Rain => 0.58,
            RainIntensity::Heavy => 1.0,
        };
        queue.write_buffer(
            buffer,
            0,
            bytemuck::bytes_of(&WeatherSurfaceUniform {
                amount_distance: [self.surface_wetness, RAIN_WETNESS_FADE_START, RAIN_WETNESS_FADE_END, intensity],
                puddle: [
                    self.puddle_amount,
                    self.surface_wetness_last_time,
                    if self.enabled { ripple_intensity } else { 0.0 },
                    if self.puddle_debug_visualization { 1.0 } else { 0.0 },
                ],
                occlusion_uv,
                occlusion_size: [width, height, active, 0],
            }),
        );
    }

    pub(crate) fn ensure_occlusion(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Option<wgpu::TextureView> {
        let cache = std::mem::replace(&mut self.occlusion, WeatherOcclusionCache::Unavailable);
        let built = match cache {
            WeatherOcclusionCache::Ready(info) => {
                self.occlusion = WeatherOcclusionCache::Ready(info);
                return None;
            }
            WeatherOcclusionCache::Unavailable => return None,
            WeatherOcclusionCache::Pending(source) => build_weather_occlusion_cpu(&source),
            WeatherOcclusionCache::Building(handle) => match handle.join() {
                Ok(result) => result,
                Err(_) => {
                    eprintln!("Weather occlusion worker panicked; weather heightfield unavailable for this map");
                    None
                }
            },
        };
        let Some(built) = built else {
            println!("Weather occlusion: no usable static BSP solid footprint");
            return None;
        };
        let basin_cells = built.surface_field.iter().filter(|cell| cell[3] > 0.01).count();
        let topography_cells = built
            .surface_field
            .iter()
            .filter(|cell| cell[1] > WEATHER_NO_SURFACE_HEIGHT + 1.0)
            .count();
        let (texture, view) = create_rain_height_texture(
            device,
            queue,
            "JKA map-wide weather occlusion heightfield",
            built.info.width,
            built.info.height,
            &built.surface_field,
        );
        let weather_height_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.gpu.sim_bind_group = create_rain_sim_bind_group(
            device,
            &self.gpu.sim_layout,
            &self.gpu.particle_buffer,
            &self.gpu.sim_uniform_buffer,
            &view,
            &self.gpu.wind_noise_view,
            &self.gpu.wind_noise_sampler,
        );
        self.gpu._collision_texture = texture;
        self.gpu.collision_view = view;
        self.occlusion = WeatherOcclusionCache::Ready(built.info);
        println!(
            "Weather surface field: background CPU solve {}x{} from {} BSP brush(es), {} blocker triangle(s), {} rendered topography triangle(s); {}/{} top-visible topography texels are puddle basins; CPU {:.1} ms",
            built.info.width, built.info.height, built.brush_count, built.triangle_count,
            built.topography_triangle_count, basin_cells, topography_cells, built.build_ms,
        );
        Some(weather_height_view)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_frame(
        &mut self,
        queue: &wgpu::Queue,
        camera: &Camera,
        frame_time: f32,
        previous_frame_time: f32,
        viewport_width: u32,
        viewport_height: u32,
        weather_wind: crate::ocean::OceanWind,
        sun_color: [f32; 3],
        sun_intensity: f32,
    ) {
        let (streak_length, streak_width, fall_speed, spawn_radius, below, above, opacity, splash_size, splash_time) = match self.intensity {
            RainIntensity::Light => (30.0, 0.68, 1_400.0, 1_000.0, 300.0, 1_050.0, 0.17, 9.0, 0.22),
            RainIntensity::Rain => (38.0, 0.82, 1_650.0, 1_075.0, 350.0, 1_175.0, 0.21, 11.5, 0.26),
            RainIntensity::Heavy => (48.0, 0.98, 1_900.0, 1_150.0, 420.0, 1_300.0, 0.24, 14.0, 0.30),
        };
        let dt = if previous_frame_time > 0.0 {
            (frame_time - previous_frame_time).clamp(0.0, 0.05)
        } else {
            1.0 / 120.0
        };
        let (collision_uv, collision_width, collision_height) = match &self.occlusion {
            WeatherOcclusionCache::Ready(info) => (
                [info.min_xz[0], info.min_xz[1], info.inv_extent_xz[0], info.inv_extent_xz[1]],
                info.width, info.height,
            ),
            WeatherOcclusionCache::Pending(_) | WeatherOcclusionCache::Building(_) | WeatherOcclusionCache::Unavailable => ([0.0; 4], 0, 0),
        };
        let sim = RainSimUniform {
            camera_dt: [camera.position.x, camera.position.y, camera.position.z, dt],
            spawn: [spawn_radius, below, above, fall_speed],
            // Resolve the same authored weather gust/veer function used by ocean.
            // rain_sim.wgsl still adds only local spatial turbulence on top.
            wind_time: {
                let wind = weather_wind.at(frame_time);
                [wind[0], wind[1], frame_time, splash_time]
            },
            weather: {
                let weather = weather_wind.sanitize();
                let angle = weather.direction.to_radians();
                [
                    angle.cos() * weather.speed,
                    angle.sin() * weather.speed,
                    0.0,
                    0.0,
                ]
            },
            collision_uv,
            counts: [
                self.intensity.particle_count().min(RAIN_MAX_PARTICLES),
                collision_width,
                collision_height,
                self.intensity.splash_particle_count().min(RAIN_MAX_PARTICLES),
            ],
        };
        queue.write_buffer(&self.gpu.sim_uniform_buffer, 0, bytemuck::bytes_of(&sim));

        let sun_scale = (sun_intensity / 250.0).max(0.0).sqrt().clamp(0.28, 1.15);
        let base = Vec3::new(0.72, 0.80, 0.90);
        let sun_color = Vec3::from_array(sun_color).max(Vec3::splat(0.05));
        let lit = (base * 0.72 + sun_color * 0.28) * sun_scale;
        let render = RainRenderUniform {
            appearance: [streak_length, streak_width, opacity, splash_size],
            splash: [splash_time, viewport_width.max(1) as f32, viewport_height.max(1) as f32, self.puddle_amount],
            color: [lit.x, lit.y, lit.z, 1.0],
        };
        queue.write_buffer(&self.gpu.render_uniform_buffer, 0, bytemuck::bytes_of(&render));
    }
}

pub(crate) fn rain_haze_mask_dimensions(viewport_width: u32, viewport_height: u32) -> (u32, u32) {
    let width = RAIN_HAZE_MASK_WIDTH.max(1);
    let aspect = viewport_height.max(1) as f32 / viewport_width.max(1) as f32;
    let height = ((width as f32) * aspect).round().clamp(
        RAIN_HAZE_MASK_MIN_HEIGHT as f32,
        RAIN_HAZE_MASK_MAX_HEIGHT as f32,
    ) as u32;
    (width, height.max(1))
}

fn create_rain_haze_resources(device: &wgpu::Device) -> RainHazeResources {
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("JKA rain haze exposure layout"),
        entries: &[
            uniform_entry(0, wgpu::ShaderStages::COMPUTE),
            unfilterable_texture_entry_compute(1),
            unfilterable_texture_entry_compute(2),
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                count: None,
            },
            filterable_texture_entry_compute(4),
            filtering_sampler_entry_compute(5),
        ],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("JKA rain haze exposure shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../rain_haze_mask.wgsl").into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("JKA rain haze exposure pipeline layout"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("JKA rain haze exposure pipeline"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("cs_main"),
        compilation_options: Default::default(),
        cache: None,
    });
    RainHazeResources { layout, bind_group: None, pipeline }
}

fn create_rain_haze_mask_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    post_buffer: &wgpu::Buffer,
    linear_depth_view: &wgpu::TextureView,
    weather_occlusion_view: &wgpu::TextureView,
    rain_haze_mask_view: &wgpu::TextureView,
    wind_noise_view: &wgpu::TextureView,
    wind_noise_sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA rain haze exposure bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: post_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(linear_depth_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(weather_occlusion_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(rain_haze_mask_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(wind_noise_view),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::Sampler(wind_noise_sampler),
            },
        ],
    })
}


pub(crate) fn start_weather_occlusion_build(source: WeatherOcclusionSource) -> WeatherOcclusionCache {
    let source = Arc::new(source);
    let worker_source = Arc::clone(&source);
    match thread::Builder::new()
        .name("jka-weather-field".into())
        .spawn(move || build_weather_occlusion_cpu(&worker_source))
    {
        Ok(handle) => WeatherOcclusionCache::Building(handle),
        Err(error) => {
            eprintln!("Could not start weather-occlusion worker: {error}; falling back to synchronous first-use build");
            WeatherOcclusionCache::Pending(source)
        }
    }
}

pub(crate) fn build_weather_occlusion_cpu(source: &WeatherOcclusionSource) -> Option<WeatherOcclusionBuildResult> {
    let started = Instant::now();
    let brush_count = source.brushes.len();
    let triangle_count = source.triangles.len();
    let topography_triangle_count = source.topography_triangles.len();
    let (info, surface_field) = build_weather_occlusion_heightfield(source)?;
    Some(WeatherOcclusionBuildResult {
        info,
        surface_field,
        brush_count,
        triangle_count,
        topography_triangle_count,
        build_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}

fn weather_occlusion_axis_size(extent: f32) -> u32 {
    if !extent.is_finite() || extent <= 1.0 {
        return WEATHER_OCCLUSION_MIN_AXIS;
    }
    ((extent / WEATHER_OCCLUSION_TARGET_TEXEL).ceil() as u32)
        .clamp(WEATHER_OCCLUSION_MIN_AXIS, WEATHER_OCCLUSION_MAX_AXIS)
}

fn build_weather_occlusion_heightfield(
    source: &WeatherOcclusionSource,
) -> Option<(WeatherOcclusionInfo, Vec<[f32; 4]>)> {
    let extent_x = source.max_xz[0] - source.min_xz[0];
    let extent_z = source.max_xz[1] - source.min_xz[1];
    if !extent_x.is_finite()
        || !extent_z.is_finite()
        || extent_x <= 1.0
        || extent_z <= 1.0
        || (source.brushes.is_empty()
            && source.triangles.is_empty()
            && source.topography_triangles.is_empty())
    {
        return None;
    }

    let width = weather_occlusion_axis_size(extent_x);
    let height = weather_occlusion_axis_size(extent_z);
    let step_x = extent_x / width as f32;
    let step_z = extent_z / height as f32;

    // RGBA32F immutable weather field, generated lazily on the first weather
    // activation for the current map:
    //   R = highest rain-blocking Y (used by rain/exposure)
    //   G = highest physical top-facing rendered surface Y
    //   B = physical upwardness of that top surface
    //   A = camera-independent coherent terrace-basin score
    //
    // This deliberately separates "blocks rain" from "can hold standing water".
    // Many authored JKA planar shader faces are real visible floors without being
    // represented by the collision subset used by the old rain heightfield.
    let mut field = vec![
        [WEATHER_NO_SURFACE_HEIGHT, WEATHER_NO_SURFACE_HEIGHT, 0.0, 0.0];
        (width as usize) * (height as usize)
    ];

    // First build the rain blocker height in R from solid/terrain brushes.
    for brush in &source.brushes {
        let brush_min_x = brush.mins_xy[0];
        let brush_max_x = brush.maxs_xy[0];
        let brush_min_z = -brush.maxs_xy[1];
        let brush_max_z = -brush.mins_xy[1];

        let px0 = (((brush_min_x - source.min_xz[0]) / step_x).floor() as i32)
            .clamp(0, width as i32 - 1) as u32;
        let px1 = (((brush_max_x - source.min_xz[0]) / step_x).ceil() as i32)
            .clamp(0, width as i32) as u32;
        let pz0 = (((brush_min_z - source.min_xz[1]) / step_z).floor() as i32)
            .clamp(0, height as i32 - 1) as u32;
        let pz1 = (((brush_max_z - source.min_xz[1]) / step_z).ceil() as i32)
            .clamp(0, height as i32) as u32;
        if px0 >= px1 || pz0 >= pz1 {
            continue;
        }

        for pz in pz0..pz1 {
            let render_z = source.min_xz[1] + (pz as f32 + 0.5) * step_z;
            let jka_y = -render_z;
            for px in px0..px1 {
                let jka_x = source.min_xz[0] + (px as f32 + 0.5) * step_x;
                let mut lower = f32::NEG_INFINITY;
                let mut upper = f32::INFINITY;
                let mut inside = true;

                for side in &brush.sides {
                    let [nx, ny, nz] = side.normal;
                    let remaining = side.distance - nx * jka_x - ny * jka_y;
                    if nz.abs() <= 1.0e-6 {
                        if remaining < -0.05 {
                            inside = false;
                            break;
                        }
                    } else {
                        let bound = remaining / nz;
                        if nz > 0.0 {
                            upper = upper.min(bound);
                        } else {
                            lower = lower.max(bound);
                        }
                    }
                }

                if !inside || !upper.is_finite() || lower > upper + 0.05 {
                    continue;
                }
                let offset = pz as usize * width as usize + px as usize;
                field[offset][0] = field[offset][0].max(upper);
            }
        }
    }

    // Tessellated patch/triangle collision supplements also block rain.
    for triangle in &source.triangles {
        let to_render_xz = |p: [f32; 3]| [p[0], -p[1]];
        let a = to_render_xz(triangle.positions[0]);
        let b = to_render_xz(triangle.positions[1]);
        let c = to_render_xz(triangle.positions[2]);
        let min_tx = a[0].min(b[0]).min(c[0]);
        let max_tx = a[0].max(b[0]).max(c[0]);
        let min_tz = a[1].min(b[1]).min(c[1]);
        let max_tz = a[1].max(b[1]).max(c[1]);
        let px0 = (((min_tx - source.min_xz[0]) / step_x).floor() as i32)
            .clamp(0, width as i32 - 1) as u32;
        let px1 = (((max_tx - source.min_xz[0]) / step_x).ceil() as i32)
            .clamp(0, width as i32) as u32;
        let pz0 = (((min_tz - source.min_xz[1]) / step_z).floor() as i32)
            .clamp(0, height as i32 - 1) as u32;
        let pz1 = (((max_tz - source.min_xz[1]) / step_z).ceil() as i32)
            .clamp(0, height as i32) as u32;
        if px0 >= px1 || pz0 >= pz1 {
            continue;
        }
        let denom = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
        if denom.abs() <= 1.0e-8 {
            continue;
        }
        for pz in pz0..pz1 {
            let z = source.min_xz[1] + (pz as f32 + 0.5) * step_z;
            for px in px0..px1 {
                let x = source.min_xz[0] + (px as f32 + 0.5) * step_x;
                let w0 = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (z - c[1])) / denom;
                let w1 = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (z - c[1])) / denom;
                let w2 = 1.0 - w0 - w1;
                if w0 < -0.001 || w1 < -0.001 || w2 < -0.001 {
                    continue;
                }
                let surface_y = w0 * triangle.positions[0][2]
                    + w1 * triangle.positions[1][2]
                    + w2 * triangle.positions[2][2];
                let offset = pz as usize * width as usize + px as usize;
                field[offset][0] = field[offset][0].max(surface_y);
            }
        }
    }

    // Build a small multi-layer topography cache for puddle classification.
    //
    // A JKA BSP is not a single terrain heightfield: floors, catwalks, roofs and
    // trims can overlap at the same X/Z. The old implementation kept only the
    // highest rendered surface per texel, which made lower-but-exposed terraces
    // fragment or disappear when a small raised insert crossed them. Keep the
    // four highest physical TOP-facing layers during this one-time build, then
    // classify connected same-height terraces as a unit.
    const PUDDLE_LAYER_CAP: usize = 4;
    const PUDDLE_LAYER_HEIGHT_TOLERANCE: f32 = 0.75;
    const PUDDLE_MIN_PHYSICAL_UPNESS: f32 = 0.985;
    const PUDDLE_BOUNDARY_HEIGHT_EPSILON: f32 = 1.5;
    const PUDDLE_MAX_RELATED_STEP: f32 = 96.0;
    const PUDDLE_MIN_COMPONENT_TEXELS: usize = 8;

    #[derive(Clone, Copy)]
    struct PuddleLayer {
        height: f32,
        upness: f32,
    }

    let empty_layer = PuddleLayer {
        height: WEATHER_NO_SURFACE_HEIGHT,
        upness: 0.0,
    };
    let mut topography_layers =
        vec![[empty_layer; PUDDLE_LAYER_CAP]; (width as usize) * (height as usize)];

    let insert_topography_layer =
        |layers: &mut [PuddleLayer; PUDDLE_LAYER_CAP], height: f32, upness: f32| {
            // Merge coplanar triangles into the same layer first.
            for layer in layers.iter_mut() {
                if layer.height > WEATHER_NO_SURFACE_HEIGHT + 1.0
                    && (layer.height - height).abs() <= PUDDLE_LAYER_HEIGHT_TOLERANCE
                {
                    layer.height = layer.height.max(height);
                    layer.upness = layer.upness.max(upness);
                    return;
                }
            }

            // Layers are maintained highest -> lowest. If the new surface is
            // below the retained capacity it is irrelevant to any rain-exposed
            // puddle and can be discarded.
            let mut insert_at = PUDDLE_LAYER_CAP;
            for (index, layer) in layers.iter().enumerate() {
                if height > layer.height {
                    insert_at = index;
                    break;
                }
            }
            if insert_at >= PUDDLE_LAYER_CAP {
                return;
            }
            for index in ((insert_at + 1)..PUDDLE_LAYER_CAP).rev() {
                layers[index] = layers[index - 1];
            }
            layers[insert_at] = PuddleLayer { height, upness };
        };

    // Rasterize every visible non-sky world surface into the temporary
    // multi-layer cache. Use signed physical winding here rather than abs(z):
    // on RBSP world faces a physical upward-facing floor has negative raw-Z
    // winding. That rejects ceiling undersides from puddle topology while still
    // allowing explicit planar shader faces such as taspir/landing_pad.
    for triangle in &source.topography_triangles {
        let ab3 = Vec3::from_array(triangle.positions[1]) - Vec3::from_array(triangle.positions[0]);
        let ac3 = Vec3::from_array(triangle.positions[2]) - Vec3::from_array(triangle.positions[0]);
        let face = ab3.cross(ac3);
        let face_len = face.length().max(1.0e-6);
        let physical_upness = (-face.z / face_len).clamp(0.0, 1.0);
        if physical_upness < PUDDLE_MIN_PHYSICAL_UPNESS {
            continue;
        }

        let to_render_xz = |p: [f32; 3]| [p[0], -p[1]];
        let a = to_render_xz(triangle.positions[0]);
        let b = to_render_xz(triangle.positions[1]);
        let c = to_render_xz(triangle.positions[2]);
        let min_tx = a[0].min(b[0]).min(c[0]);
        let max_tx = a[0].max(b[0]).max(c[0]);
        let min_tz = a[1].min(b[1]).min(c[1]);
        let max_tz = a[1].max(b[1]).max(c[1]);
        let px0 = (((min_tx - source.min_xz[0]) / step_x).floor() as i32)
            .clamp(0, width as i32 - 1) as u32;
        let px1 = (((max_tx - source.min_xz[0]) / step_x).ceil() as i32)
            .clamp(0, width as i32) as u32;
        let pz0 = (((min_tz - source.min_xz[1]) / step_z).floor() as i32)
            .clamp(0, height as i32 - 1) as u32;
        let pz1 = (((max_tz - source.min_xz[1]) / step_z).ceil() as i32)
            .clamp(0, height as i32) as u32;
        if px0 >= px1 || pz0 >= pz1 {
            continue;
        }
        let denom = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
        if denom.abs() <= 1.0e-8 {
            continue;
        }

        for pz in pz0..pz1 {
            let z = source.min_xz[1] + (pz as f32 + 0.5) * step_z;
            for px in px0..px1 {
                let x = source.min_xz[0] + (px as f32 + 0.5) * step_x;
                let w0 = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (z - c[1])) / denom;
                let w1 = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (z - c[1])) / denom;
                let w2 = 1.0 - w0 - w1;
                if w0 < -0.001 || w1 < -0.001 || w2 < -0.001 {
                    continue;
                }
                let surface_y = w0 * triangle.positions[0][2]
                    + w1 * triangle.positions[1][2]
                    + w2 * triangle.positions[2][2];
                let offset = pz as usize * width as usize + px as usize;
                insert_topography_layer(
                    &mut topography_layers[offset],
                    surface_y,
                    physical_upness,
                );
            }
        }
    }

    // Publish the highest physical top-facing layer for shader height matching.
    // The basin score in A is filled below for qualifying geometric terraces;
    // ordinary wet-film exposure still uses the independent blocker R and the
    // material shader applies cover against the actual fragment height.
    for (offset, layers) in topography_layers.iter().enumerate() {
        let top = layers[0];
        if top.height > WEATHER_NO_SURFACE_HEIGHT + 1.0 {
            field[offset][1] = top.height;
            field[offset][2] = top.upness;
        }
    }

    let layer_valid = |layer: PuddleLayer| -> bool {
        layer.height > WEATHER_NO_SURFACE_HEIGHT + 1.0 && layer.height.is_finite()
    };
    let find_matching_layer = |cell: usize, target_height: f32| -> Option<usize> {
        let mut best_index = None;
        let mut best_delta = f32::INFINITY;
        for (index, layer) in topography_layers[cell].iter().enumerate() {
            if !layer_valid(*layer) {
                continue;
            }
            let delta = (layer.height - target_height).abs();
            if delta < best_delta {
                best_delta = delta;
                best_index = Some(index);
            }
        }
        if best_delta <= PUDDLE_LAYER_HEIGHT_TOLERANCE {
            best_index
        } else {
            None
        }
    };

    // Classify coherent flat terraces instead of individual texels. A terrace
    // qualifies only when essentially its entire perimeter is retained by
    // nearby higher floor and it has almost no lower/open drainage edges. These
    // thresholds match the world-model-only classifier verified offline against
    // ffa5.bsp: landing_pad forms one coherent -16-unit terrace with a 0-unit
    // spill rim instead of sparse islands.
    let mut visited = vec![false; topography_layers.len() * PUDDLE_LAYER_CAP];
    let mut stack = Vec::<usize>::new();
    let mut members = Vec::<usize>::new();
    let mut higher_deltas = Vec::<f32>::new();
    let grid_width = width as usize;
    let grid_height = height as usize;

    let smooth_unit = |value: f32, low: f32, high: f32| -> f32 {
        let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };

    for cell in 0..topography_layers.len() {
        for layer_index in 0..PUDDLE_LAYER_CAP {
            let node = cell * PUDDLE_LAYER_CAP + layer_index;
            if visited[node] || !layer_valid(topography_layers[cell][layer_index]) {
                continue;
            }

            visited[node] = true;
            stack.clear();
            members.clear();
            stack.push(node);

            while let Some(current) = stack.pop() {
                members.push(current);
                let current_cell = current / PUDDLE_LAYER_CAP;
                let current_layer = current % PUDDLE_LAYER_CAP;
                let current_height = topography_layers[current_cell][current_layer].height;
                let x = current_cell % grid_width;
                let y = current_cell / grid_width;

                let neighbors = [
                    (x as i32 - 1, y as i32),
                    (x as i32 + 1, y as i32),
                    (x as i32, y as i32 - 1),
                    (x as i32, y as i32 + 1),
                ];
                for (nx, ny) in neighbors {
                    if nx < 0 || ny < 0 || nx >= grid_width as i32 || ny >= grid_height as i32 {
                        continue;
                    }
                    let neighbor_cell = ny as usize * grid_width + nx as usize;
                    let Some(neighbor_layer) = find_matching_layer(neighbor_cell, current_height) else {
                        continue;
                    };
                    let neighbor_node = neighbor_cell * PUDDLE_LAYER_CAP + neighbor_layer;
                    if !visited[neighbor_node] {
                        visited[neighbor_node] = true;
                        stack.push(neighbor_node);
                    }
                }
            }

            if members.len() < PUDDLE_MIN_COMPONENT_TEXELS {
                continue;
            }

            let mut total_edges = 0_u32;
            let mut lower_edges = 0_u32;
            let mut open_edges = 0_u32;
            higher_deltas.clear();

            for &member in &members {
                let member_cell = member / PUDDLE_LAYER_CAP;
                let member_layer = member % PUDDLE_LAYER_CAP;
                let terrace_height = topography_layers[member_cell][member_layer].height;
                let x = member_cell % grid_width;
                let y = member_cell / grid_width;
                let neighbors = [
                    (x as i32 - 1, y as i32),
                    (x as i32 + 1, y as i32),
                    (x as i32, y as i32 - 1),
                    (x as i32, y as i32 + 1),
                ];

                for (nx, ny) in neighbors {
                    if nx >= 0 && ny >= 0 && nx < grid_width as i32 && ny < grid_height as i32 {
                        let neighbor_cell = ny as usize * grid_width + nx as usize;
                        if find_matching_layer(neighbor_cell, terrace_height).is_some() {
                            // Same terrace continues through this edge, including
                            // beneath a small higher insert/trim layer.
                            continue;
                        }
                    }

                    total_edges += 1;
                    if nx < 0 || ny < 0 || nx >= grid_width as i32 || ny >= grid_height as i32 {
                        open_edges += 1;
                        continue;
                    }

                    let neighbor_cell = ny as usize * grid_width + nx as usize;
                    let mut closest_height = None;
                    let mut closest_delta = f32::INFINITY;
                    for layer in topography_layers[neighbor_cell] {
                        if !layer_valid(layer) {
                            continue;
                        }
                        let delta = (layer.height - terrace_height).abs();
                        if delta <= PUDDLE_MAX_RELATED_STEP && delta < closest_delta {
                            closest_delta = delta;
                            closest_height = Some(layer.height);
                        }
                    }

                    let Some(neighbor_height) = closest_height else {
                        open_edges += 1;
                        continue;
                    };
                    let delta = neighbor_height - terrace_height;
                    if delta > PUDDLE_BOUNDARY_HEIGHT_EPSILON {
                        higher_deltas.push(delta);
                    } else if delta < -PUDDLE_BOUNDARY_HEIGHT_EPSILON {
                        lower_edges += 1;
                    }
                }
            }

            if total_edges == 0 || higher_deltas.is_empty() {
                continue;
            }

            let higher_fraction = higher_deltas.len() as f32 / total_edges as f32;
            let drain_fraction = (lower_edges + open_edges) as f32 / total_edges as f32;
            higher_deltas.sort_by(|a, b| a.total_cmp(b));
            let spill_index = (((higher_deltas.len() - 1) as f32) * 0.10).floor() as usize;
            let spill_depth = higher_deltas[spill_index];

            if higher_fraction < 0.78 || drain_fraction > 0.05 || spill_depth < 2.0 {
                continue;
            }

            // Depth controls how soon the basin becomes visibly flooded as the
            // global rain accumulation rises. A 16-unit landing_pad depression
            // reaches full score; a very shallow lip only appears under heavy rain.
            let basin_score = smooth_unit(spill_depth, 1.5, 10.0);

            for &member in &members {
                let member_cell = member / PUDDLE_LAYER_CAP;
                let member_layer = member % PUDDLE_LAYER_CAP;
                // Only the highest physical top-facing surface at this X/Z is
                // eligible to receive direct rainfall. Lower terraces remain in
                // the component solve so small overlying inserts do not fragment
                // the basin, but they are never rendered as puddles through cover.
                if member_layer != 0 {
                    continue;
                }

                // Store the geometric basin result exactly as classified. Rain
                // exposure is deliberately NOT baked into the basin channel: the
                // material shader already evaluates the shared cover height against
                // the actual fragment height every frame. Baking exposure here was
                // both redundant and could erase a valid recessed terrace when the
                // collision roof field differed slightly from the rendered floor.
                field[member_cell][3] = field[member_cell][3].max(basin_score);
            }
        }
    }

    Some((
        WeatherOcclusionInfo {
            min_xz: source.min_xz,
            inv_extent_xz: [1.0 / extent_x, 1.0 / extent_z],
            width,
            height,
        },
        field,
    ))
}

pub(crate) fn create_rain_height_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &'static str,
    width: u32,
    height: u32,
    data: &[[f32; 4]],
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        bytemuck::cast_slice(data),
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

pub(crate) fn create_rain_sim_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    particle_buffer: &wgpu::Buffer,
    sim_uniform_buffer: &wgpu::Buffer,
    collision_view: &wgpu::TextureView,
    wind_noise_view: &wgpu::TextureView,
    wind_noise_sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA GPU rain simulation bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: particle_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: sim_uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(collision_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(wind_noise_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(wind_noise_sampler),
            },
        ],
    })
}

fn create_rain_gpu_resources(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    camera_layout: &wgpu::BindGroupLayout,
    surface_format: wgpu::TextureFormat,
    sample_count: u32,
    wind_noise_texture: &wgpu::Texture,
) -> RainGpuResources {
    // A 1x1 no-surface texture keeps the compute layout valid before the first
    // rain activation. The real map-wide heightfield is allocated lazily.
    let (collision_texture, collision_view) = create_rain_height_texture(
        device,
        queue,
        "JKA rain occlusion placeholder",
        1,
        1,
        &[[WEATHER_NO_SURFACE_HEIGHT, WEATHER_NO_SURFACE_HEIGHT, 0.0, 0.0]],
    );

    // Reuse GrassRenderer's canonical 512x512 GodotGrass wind texture. Rain
    // owns only a view and sampler; there is no second Perlin/domain-warp bake.
    let wind_noise_view = wind_noise_texture.create_view(&wgpu::TextureViewDescriptor::default());
    let wind_noise_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("JKA shared weather wind sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    });

    let particles = vec![
        RainParticle {
            position_state: [0.0; 4],
            velocity_age: [0.0; 4],
            misc: [0.0; 4],
        };
        RAIN_MAX_PARTICLES as usize
    ];
    let particle_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA GPU rain particle state"),
        contents: bytemuck::cast_slice(&particles),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    });

    let sim_uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA GPU rain simulation uniform"),
        contents: bytemuck::bytes_of(&RainSimUniform {
            camera_dt: [0.0, 0.0, 0.0, 1.0 / 120.0],
            spawn: [1_075.0, 350.0, 1_175.0, 1_650.0],
            wind_time: [0.0, 0.0, 0.0, 0.26],
            weather: [0.0, 0.0, 0.0, 0.0],
            collision_uv: [0.0; 4],
            counts: [14_000, 0, 0, 3_500],
        }),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let sim_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("JKA GPU rain simulation layout"),
        entries: &[
            storage_buffer_entry(0, false, wgpu::ShaderStages::COMPUTE),
            uniform_entry(1, wgpu::ShaderStages::COMPUTE),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            filterable_texture_entry_compute(3),
            filtering_sampler_entry_compute(4),
        ],
    });
    let sim_bind_group = create_rain_sim_bind_group(
        device,
        &sim_layout,
        &particle_buffer,
        &sim_uniform_buffer,
        &collision_view,
        &wind_noise_view,
        &wind_noise_sampler,
    );
    let sim_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("JKA GPU rain compute shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../rain_sim.wgsl").into()),
    });
    let sim_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("JKA GPU rain simulation pipeline layout"),
        bind_group_layouts: &[Some(&sim_layout)],
        immediate_size: 0,
    });
    let sim_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("JKA GPU rain simulation pipeline"),
        layout: Some(&sim_pipeline_layout),
        module: &sim_shader,
        entry_point: Some("cs_main"),
        compilation_options: Default::default(),
        cache: None,
    });

    let render_uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA GPU rain render uniform"),
        contents: bytemuck::bytes_of(&RainRenderUniform {
            appearance: [18.0, 1.40, 0.21, 11.5],
            splash: [0.26, 1.0, 1.0, 0.0],
            color: [0.82, 0.88, 0.94, 1.0],
        }),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let render_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("JKA GPU rain render layout"),
        entries: &[
            storage_buffer_entry(0, true, wgpu::ShaderStages::VERTEX),
            uniform_entry(1, wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT),
        ],
    });
    let render_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA GPU rain render bind group"),
        layout: &render_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: particle_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: render_uniform_buffer.as_entire_binding(),
            },
        ],
    });
    let render_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("JKA GPU rain render pipeline layout"),
        bind_group_layouts: &[Some(camera_layout), Some(&render_layout)],
        immediate_size: 0,
    });
    let render_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("JKA GPU rain render shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../rain.wgsl").into()),
    });
    let drop_pipeline = create_rain_drop_pipeline(
        device,
        &render_pipeline_layout,
        &render_shader,
        surface_format,
        sample_count,
    );
    let splash_pipeline = create_rain_splash_pipeline(
        device,
        &render_pipeline_layout,
        &render_shader,
        surface_format,
        sample_count,
    );

    RainGpuResources {
        _collision_texture: collision_texture,
        collision_view,
        wind_noise_view,
        wind_noise_sampler,
        particle_buffer,
        sim_uniform_buffer,
        sim_layout,
        sim_bind_group,
        sim_pipeline,
        render_uniform_buffer,
        render_bind_group,
        render_pipeline_layout,
        render_shader,
        drop_pipeline,
        splash_pipeline,
    }
}

fn rain_depth_state() -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(false),
        depth_compare: Some(camera_depth_compare()),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

fn create_rain_drop_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    sample_count: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA GPU rain drop pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_drop"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(rain_depth_state()),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_drop"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn create_rain_splash_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    sample_count: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA GPU rain splash pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_splash"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(rain_depth_state()),
        multisample: wgpu::MultisampleState {
            count: sample_count,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_splash"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

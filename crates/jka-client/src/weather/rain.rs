use crate::{
    camera::Camera,
    pipeline_jobs::{hash_config as pipeline_hash, PipelineJobKey, PipelineJobManager},
    scene::WeatherOcclusionSource,
    ui::{PuddleQuality, RainIntensity},
};
use super::{
    cpu_field::CpuField,
    rain_glass::RainGlassResources,
    wake::{WakeEvent, WakeTracker, WAKE_LIFETIME, WAKE_MAX_EVENTS, WAKE_RING_SPEED},
};
use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use jka_assets::pk3::AssetSearchPath;
use std::{
    sync::Arc,
    thread::{self, JoinHandle},
    time::Instant,
};
use wgpu::util::DeviceExt;

pub(crate) const RAIN_MAX_PARTICLES: u32 = 32_768;
const MAX_PARTICLE_CLOUDS_COMPAT: u32 = crate::world_effects::MAX_PARTICLE_CLOUDS as u32;
pub(crate) const RAIN_COMPUTE_GROUP_SIZE: u32 = 128;
pub(crate) const RAIN_HAZE_MASK_WIDTH: u32 = 64;
pub(crate) const RAIN_HAZE_MASK_MIN_HEIGHT: u32 = 16;
pub(crate) const RAIN_HAZE_MASK_MAX_HEIGHT: u32 = 64;
pub(crate) const WEATHER_NO_SURFACE_HEIGHT: f32 = -1.0e20;
// Grass reads this short range from the shared uniform: its per-blade wetness is
// only worth evaluating in the near field.
pub(crate) const RAIN_WETNESS_FADE_START: f32 = 450.0;
pub(crate) const RAIN_WETNESS_FADE_END: f32 = 1150.0;
// Ground and walls stay wet much farther out so a wet street keeps reflecting all
// the way down its length instead of turning dry a block away.
const RAIN_FILM_FADE_START: f32 = 1400.0;
const RAIN_FILM_FADE_END: f32 = 4200.0;
// Extra reflectivity of a wet film under High puddle water.
const RAIN_FILM_GLOSS_HIGH: f32 = 1.5;
// Where the puddle coat hands its environment over from the reflection probe to
// the map's skybox, as the vertical component of the mirror ray, and the largest
// share the skybox is ever allowed. See `weather_coat_sky_share` in
// weather_surface_world.wgsl for the geometry these came from; `puddle_sky` edits
// them live so the look can be judged from a screenshot instead of guessed.
pub(crate) const RAIN_SKY_BLEND_WINDOW: f32 = 0.60;
pub(crate) const RAIN_SKY_BLEND_CEILING: f32 = 0.70;
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
    /// Wetness, grass fade start/end, rain-intensity response.
    pub amount_distance: [f32; 4],
    /// Puddle accumulation, ripple time, ripple strength, debug view mode.
    pub puddle: [f32; 4],
    pub occlusion_uv: [f32; 4],
    pub occlusion_size: [u32; 4],
    /// Scattered puddle amount, reflection streak strength, high-quality water,
    /// wet-film gloss.
    pub look: [f32; 4],
    /// Wet-film fade start/end in world units, then the coat's probe-to-skybox
    /// handover window and the skybox's share ceiling.
    pub film_fade: [f32; 4],
    /// Light travel direction (render space) and relative intensity (250 = 1).
    pub sun_direction: [f32; 4],
    pub sun_radiance: [f32; 4],
    /// Render-space wind X/Z in world units per second.
    pub wind: [f32; 4],
    /// Wake event count, lifetime, ring speed and the current time.
    pub wake_info: [f32; 4],
    pub wake_a: [[f32; 4]; WAKE_MAX_EVENTS],
    pub wake_b: [[f32; 4]; WAKE_MAX_EVENTS],
}

/// Frame environment the surface shaders need besides the rain state itself.
pub(crate) struct WeatherSurfaceEnvironment<'a> {
    pub sun_direction: [f32; 3],
    pub sun_color: [f32; 3],
    pub sun_intensity: f32,
    pub wind: [f32; 2],
    /// The map has a real skybox for the water to mirror.
    pub has_sky: bool,
    pub wake: &'a [WakeEvent],
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
    /// stable/base weather wind X/Z; horizontal camera delta X/Z.
    weather: [f32; 4],
    collision_uv: [f32; 4],
    counts: [u32; 4],
    /// x: legacy water-volume count; y: prebaked water-field A/B;
    /// z: fast bounds/vertex A/B. Rain lands on the top surface.
    water_info: [f32; 4],
    water_min: [[f32; 4]; RAIN_MAX_WATER_VOLUMES],
    water_max: [[f32; 4]; RAIN_MAX_WATER_VOLUMES],
}
/// Outlines of the world's promoted water surfaces (`OceanMasks`), so a water
/// volume's box can have holes. Uploaded only when the world's masks change.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RainWaterMasks {
    ranges: [[f32; 4]; crate::ocean::OCEAN_MAX_SURFACES],
    bounds: [[f32; 4]; crate::ocean::OCEAN_MAX_SURFACES],
    tiles: [[u32; 4]; crate::ocean::OCEAN_MAX_SURFACES],
    triangles: [[f32; 4]; crate::ocean::OCEAN_MASK_VEC4S],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RainRenderUniform {
    appearance: [f32; 4],
    splash: [f32; 4],
    color: [f32; 4],
    /// Scattered puddle amount for splash rings, 1 while the camera is under
    /// water (all rain is then above the surface and must not be drawn), reserved.
    look: [f32; 4],
}

const RAIN_MAX_WATER_VOLUMES: usize = 8;

// How quickly the camera's exposure settles when it walks under or out from cover.
const CAMERA_EXPOSURE_SECONDS: f32 = 0.6;

fn rain_glass_wind_lens(
    camera: &Camera,
    weather_wind: crate::ocean::AtmosphericWind,
    frame_time: f32,
) -> [f32; 4] {
    let forward = camera.forward();
    let mut right = forward.cross(Vec3::Y).normalize_or_zero();
    if right.length_squared() <= f32::EPSILON {
        let (sin_yaw, cos_yaw) = camera.yaw.sin_cos();
        right = Vec3::new(sin_yaw, 0.0, cos_yaw);
    }
    let up = right.cross(forward).normalize_or_zero();
    let wind = weather_wind.at(frame_time);
    let wind_world = Vec3::new(wind[0], 0.0, wind[1]);
    [
        wind_world.dot(right),
        wind_world.dot(up),
        wind_world.dot(-forward),
        wind_world.length(),
    ]
}

#[derive(Clone, Copy)]
pub(crate) struct WeatherOcclusionInfo { pub min_xz: [f32; 2], pub inv_extent_xz: [f32; 2], pub width: u32, pub height: u32 }

pub(crate) struct WeatherOcclusionBuildResult {
    pub info: WeatherOcclusionInfo, pub surface_field: Vec<[f32; 4]>,
    /// Mip level 1 of the field texture: blurred cover statistics for the soft rain-shadow edge.
    pub shelter_level: Vec<[f32; 4]>,
    pub brush_count: usize, pub triangle_count: usize, pub topography_triangle_count: usize, pub build_ms: f64,
}

pub(crate) enum WeatherOcclusionCache {
    Unavailable, Pending(Arc<WeatherOcclusionSource>), Building(JoinHandle<Option<WeatherOcclusionBuildResult>>), Ready(WeatherOcclusionInfo),
}

pub(crate) struct RainGpuResources {
    pub _collision_texture: wgpu::Texture,
    pub collision_view: wgpu::TextureView,
    /// Map-space [water bottom, water top, 0, 0] field used by the optional
    /// O(1) rain/water collision A/B path. Same XZ grid as the weather field.
    pub _water_height_texture: wgpu::Texture,
    pub water_height_view: wgpu::TextureView,
    // A view of GrassRenderer's canonical GodotGrass wind texture plus a cheap
    // repeat/linear sampler. The underlying texture is owned only once by grass.
    pub wind_noise_view: wgpu::TextureView,
    pub wind_noise_sampler: wgpu::Sampler,
    pub particle_buffer: wgpu::Buffer,
    pub sim_uniform_buffer: wgpu::Buffer,
    pub water_mask_buffer: wgpu::Buffer,
    pub sim_layout: wgpu::BindGroupLayout,
    pub sim_bind_group: wgpu::BindGroup,
    pub sim_pipeline: wgpu::ComputePipeline,
    pub render_uniform_buffer: wgpu::Buffer,
    pub render_bind_group: wgpu::BindGroup,
    pub render_pipeline_layout: wgpu::PipelineLayout,
    pub render_shader: wgpu::ShaderModule,
    /// Compiled on first use while rain is enabled (see `ensure_render_pipelines`).
    pub drop_pipeline: Option<wgpu::RenderPipeline>,
    pub splash_pipeline: Option<wgpu::RenderPipeline>,
    pub render_format: wgpu::TextureFormat,
    pub render_samples: u32,
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
    pub native_precipitation: Option<f32>,
    pub intensity: RainIntensity,
    /// Runtime-only multiplier used by stacked legacy `r_we` rain clouds.
    pub density_scale: f32,
    /// Runtime-only RGB color selected by the legacy effect (acid rain).
    pub tint: [f32; 3],
    /// Additional post-process atmosphere applied when legacy acid rain is selected.
    pub acid_strength: f32,
    /// `r_we freeze`: keep rendering the current particles without advancing them.
    pub frozen: bool,
    pub surface_wetness: f32,
    pub puddle_amount: f32,
    /// `puddle_debug <mode>`: 0 off, otherwise the diagnostic view the world
    /// shaders paint instead of the lit colour (`weather_surface_debug`).
    pub puddle_debug_mode: u32,
    /// Live tuning for the coat's probe/skybox handover, defaulted from
    /// `RAIN_SKY_BLEND_WINDOW`/`_CEILING`.
    pub sky_blend_window: f32,
    pub sky_blend_ceiling: f32,
    pub puddle_quality: PuddleQuality,
    /// How readily rain collects in scattered puddles on large flat ground, 0..1.
    pub puddle_scatter: f32,
    /// User strength of the wet-weather colour grade, 0..1.
    pub wet_grade: f32,
    /// CPU copy of the weather field: camera exposure for the grade, and whether
    /// a footstep landed in water.
    cpu_field: Option<CpuField>,
    /// The outlines last uploaded to `gpu.water_mask_buffer`.
    water_masks: Arc<crate::ocean::OceanMasks>,
    /// Promoted water volumes used to bake the optional constant-time collision field.
    water_volumes: Vec<crate::ocean::WaterVolume>,
    /// Rebuild the water field on the next prepared rain frame. Set when the map,
    /// promoted-water set, outline masks or A/B mode changes.
    pub(crate) water_field_dirty: bool,
    /// A/B: remove rain as a reason for depth-aware haze/full spatial post.
    pub fast_post: bool,
    /// A/B: one texture load per drop instead of per-drop water triangle scans.
    pub water_field_enabled: bool,
    /// A/B: squared compute bounds + early drop center clip rejection.
    pub fast_vertex: bool,
    /// A/B: when rain is the only depth consumer, write only linear depth + hardware depth.
    pub lean_depth: bool,
    /// A/B: omit dynamic-model geometry from a rain-only linear-depth prepass.
    pub static_depth: bool,
    /// A/B: omit alpha-tested BSP from a rain-only linear-depth prepass.
    pub opaque_depth: bool,
    /// A/B: include dynamic entities in lean depth with one-target pipelines.
    pub lean_models: bool,
    /// A/B: specialized rain-only fullscreen post path.
    pub lean_post: bool,
    /// Smoothed 0..1 exposure of the camera to the rain (0 indoors).
    camera_exposure: f32,
    /// Previous precipitation anchor. Falling drops follow normal horizontal
    /// camera translation, while impact splashes remain fixed to world surfaces.
    /// This is the usual camera-relative precipitation-volume trick and prevents
    /// a sparse leading edge when the player walks through the emitter.
    camera_anchor_xz: Option<[f32; 2]>,
    pub wake: WakeTracker,
    pub surface_wetness_last_time: f32,
    /// Lazily allocated on first exposed-rain frame. This is the native WGPU
    /// port of Toadstorm/RainyGlassShader's fullscreen GrabPass refraction.
    glass: Option<RainGlassResources>,
    glass_disabled: bool,
    /// Setup > Environment > Rain on lens. Separate from `enabled` so the lens
    /// can be turned off without losing the rain itself.
    pub glass_enabled: bool,
}

impl RainSystem {
    /// Share the existing map roof field with atmosphere/water; no second bake.
    pub(crate) fn sky_admission(&self) -> (wgpu::TextureView, [f32; 4]) {
        let mut uv = match &self.occlusion {
            WeatherOcclusionCache::Ready(info) => [info.min_xz[0], info.min_xz[1], info.inv_extent_xz[0], info.inv_extent_xz[1]],
            _ => [0.0; 4],
        };
        // Same-executable performance comparison. Keep the field built/bound in
        // both runs; only the shader's admission lookups and lighting gates differ.
        static TEST_OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *TEST_OFF.get_or_init(|| std::env::var("JKA_SKY_ADMISSION_TEST_OFF").as_deref() == Ok("1")) {
            uv = [0.0; 4];
        }
        (self.gpu.collision_view.clone(), uv)
    }
    pub(crate) fn native_sheltered(&self,position:[f32;3])->bool {
        self.cpu_field.as_ref().is_some_and(|field|field.exposure_at(position)<0.15)
    }

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
            native_precipitation: None,
            intensity: RainIntensity::Rain,
            density_scale: 1.0,
            tint: [1.0; 3],
            acid_strength: 0.0,
            frozen: false,
            surface_wetness: 0.0,
            puddle_amount: 0.0,
            puddle_debug_mode: 0,
            sky_blend_window: RAIN_SKY_BLEND_WINDOW,
            sky_blend_ceiling: RAIN_SKY_BLEND_CEILING,
            puddle_quality: PuddleQuality::default(),
            puddle_scatter: 0.0,
            wet_grade: 0.0,
            cpu_field: None,
            water_masks: Arc::new(crate::ocean::OceanMasks::default()),
            water_volumes: Vec::new(),
            water_field_dirty: true,
            fast_post: false,
            water_field_enabled: false,
            fast_vertex: false,
            lean_depth: false,
            static_depth: false,
            opaque_depth: false,
            lean_models: false,
            lean_post: false,
            camera_exposure: 1.0,
            camera_anchor_xz: None,
            wake: WakeTracker::default(),
            surface_wetness_last_time: 0.0,
            glass: None,
            glass_disabled: false,
            glass_enabled: true,
        }
    }

    /// Rain-intensity response shared by the wet film and the colour grade.
    fn intensity_response(&self) -> f32 {
        if let Some(rain)=self.native_precipitation {return rain;}
        match self.intensity {
            RainIntensity::Light => 0.72,
            RainIntensity::Rain => 0.88,
            RainIntensity::Heavy => 1.0,
        }
    }

    /// How hard rain is reaching the camera itself: the rain's own intensity faded
    /// by the camera's exposure to it, so it drops off a second or two after
    /// stepping under a roof. Deliberately not `grade_amount`, which is scaled by
    /// `r_rainGrade` - a colour preference a player may turn off without meaning
    /// anything by it.
    ///
    /// This drives the Toadstorm RainyGlass lens pass and is also published in
    /// `weather_look.w` so all weather consumers share the same camera exposure.
    pub(crate) fn lens_drop_amount(&self) -> f32 {
        if !self.enabled {
            return 0.0;
        }
        (self.intensity_response() * self.camera_exposure).clamp(0.0, 1.0)
    }

    pub(crate) fn prepare_lens_glass(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        frame_time: f32,
        camera: &Camera,
        weather_wind: crate::ocean::AtmosphericWind,
        mut assets: Option<&mut AssetSearchPath>,
    ) {
        if self.glass_disabled {
            return;
        }
        let amount = if self.glass_enabled {
            self.lens_drop_amount()
        } else {
            0.0
        };
        // Do not allocate the two authored masks, scratch target or PSO until
        // rain actually reaches the camera at least once.
        if amount <= 0.001 && self.glass.is_none() {
            return;
        }
        if self.glass.is_none() {
            let Some(assets) = assets.as_deref_mut() else {
                self.glass_disabled = true;
                eprintln!("[JKA WEATHER] RainyGlass disabled: asset VFS unavailable");
                return;
            };
            match RainGlassResources::new(device, queue, width, height, format, assets) {
                Ok(glass) => self.glass = Some(glass),
                Err(error) => {
                    self.glass_disabled = true;
                    eprintln!("[JKA WEATHER] RainyGlass disabled: {error}");
                    return;
                }
            }
        }
        let glass = self
            .glass
            .as_mut()
            .expect("RainyGlass resources initialized");

        // Put the same instantaneous gust/veer vector used by the rain particles
        // into camera/lens space.  Do not integrate this into a UV offset: doing
        // that pans the entire authored RainyGlass image as one sheet.  The lens
        // shader instead applies these components only to each drop/rivulet's
        // local distortion/flow.
        let forward = camera.forward();
        let wind_lens = rain_glass_wind_lens(camera, weather_wind, frame_time);

        glass.prepare(
            jobs,
            device,
            queue,
            width,
            height,
            format,
            frame_time,
            amount,
            forward.y,
            wind_lens,
        );
    }

    pub(crate) fn lens_glass_active(&self) -> bool {
        self.enabled
            && self.glass_enabled
            && !self.glass_disabled
            && self.glass.as_ref().is_some_and(RainGlassResources::active)
    }

    pub(crate) fn lens_glass_source_view(&self) -> Option<&wgpu::TextureView> {
        self.lens_glass_active().then(|| {
            self.glass
                .as_ref()
                .expect("active RainyGlass resources")
                .source_view()
        })
    }

    pub(crate) fn render_lens_glass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
    ) {
        if !self.enabled || !self.glass_enabled || self.glass_disabled {
            return;
        }
        if let Some(glass) = self.glass.as_ref() {
            glass.render(encoder, output);
        }
    }

    /// How wet entities are (model_weather.wgsl). Models cannot read the weather
    /// field - they have no binding for it - so the camera's own exposure to the
    /// rain stands in for per-entity cover: right whenever the camera and the
    /// model share shelter, which is the case a player notices.
    pub(crate) fn entity_wetness(&self) -> f32 {
        (self.surface_wetness * self.intensity_response() * self.camera_exposure).clamp(0.0, 1.0)
    }

    /// How far the wet-weather colour grade is faded in: it follows the surface
    /// wetness, so it eases in as the ground darkens and out as it dries, and the
    /// camera's exposure, so it is absent indoors.
    pub(crate) fn grade_amount(&self) -> f32 {
        (self.wet_grade
            * self.surface_wetness
            * self.intensity_response()
            * self.camera_exposure)
            .clamp(0.0, 1.0)
    }

    pub(crate) fn start_map_occlusion(&mut self, source: Option<WeatherOcclusionSource>) {
        self.occlusion = source
            .map(start_weather_occlusion_build)
            .unwrap_or(WeatherOcclusionCache::Unavailable);
        self.cpu_field = None;
        self.water_volumes.clear();
        self.water_masks = Arc::new(crate::ocean::OceanMasks::default());
        self.water_field_dirty = true;
        self.camera_exposure = 1.0;
        self.wake.clear();
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
        // Rain is off by default; drop the stale pipelines and let
        // `ensure_render_pipelines` recompile them only while rain is enabled.
        let _ = device;
        self.gpu.render_format = surface_format;
        self.gpu.render_samples = sample_count;
        self.gpu.drop_pipeline = None;
        self.gpu.splash_pipeline = None;
    }

    pub(crate) fn ensure_render_pipelines(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
    ) {
        let config = pipeline_hash(&(self.gpu.render_format, self.gpu.render_samples));
        if self.gpu.drop_pipeline.is_none() {
            let key = PipelineJobKey::new("rain", 0, config);
            if let Some(pipeline) = jobs.take_ready(key) {
                self.gpu.drop_pipeline = Some(pipeline);
            } else {
                let device = device.clone();
                let layout = self.gpu.render_pipeline_layout.clone();
                let shader = self.gpu.render_shader.clone();
                let format = self.gpu.render_format;
                let samples = self.gpu.render_samples;
                jobs.request(key, "rain drops", move || {
                    create_rain_drop_pipeline(&device, &layout, &shader, format, samples)
                });
            }
        }
        if self.gpu.splash_pipeline.is_none() {
            let key = PipelineJobKey::new("rain", 1, config);
            if let Some(pipeline) = jobs.take_ready(key) {
                self.gpu.splash_pipeline = Some(pipeline);
            } else {
                let device = device.clone();
                let layout = self.gpu.render_pipeline_layout.clone();
                let shader = self.gpu.render_shader.clone();
                let format = self.gpu.render_format;
                let samples = self.gpu.render_samples;
                jobs.request(key, "rain splashes", move || {
                    create_rain_splash_pipeline(&device, &layout, &shader, format, samples)
                });
            }
        }
    }

    fn particle_count(&self) -> u32 {
        let base = self.intensity.particle_count() as f32;
        let scale = if self.density_scale.is_finite() {
            self.density_scale.clamp(0.0, MAX_PARTICLE_CLOUDS_COMPAT as f32)
        } else {
            1.0
        };
        (base * scale).round().clamp(0.0, RAIN_MAX_PARTICLES as f32) as u32
    }

    fn splash_particle_count(&self) -> u32 {
        let base = self.intensity.splash_particle_count() as f32;
        let scale = if self.density_scale.is_finite() {
            self.density_scale.clamp(0.0, MAX_PARTICLE_CLOUDS_COMPAT as f32)
        } else {
            1.0
        };
        (base * scale).round().clamp(0.0, self.particle_count() as f32) as u32
    }

    pub(crate) fn collision_view(&self) -> &wgpu::TextureView {
        &self.gpu.collision_view
    }

    pub(crate) fn dispatch_simulation(&self, encoder: &mut wgpu::CommandEncoder) {
        if !self.enabled || self.frozen {
            return;
        }
        let count = self.particle_count();
        if count == 0 {
            return;
        }
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
        let (Some(drop_pipeline), Some(splash_pipeline)) =
            (self.gpu.drop_pipeline.as_ref(), self.gpu.splash_pipeline.as_ref())
        else {
            return;
        };
        let count = self.particle_count();
        pass.set_pipeline(drop_pipeline);
        pass.set_bind_group(0, camera_bind_group, &[]);
        pass.set_bind_group(1, &self.gpu.render_bind_group, &[]);
        pass.draw(0..4, 0..count);

        pass.set_pipeline(splash_pipeline);
        let splash_count = self.splash_particle_count().min(count);
        pass.draw(0..6, 0..splash_count);
    }

    /// Advances the footstep/wake tracker. `people` are render-space sole positions
    /// of everyone who may wade. Tracking only runs while there is standing water
    /// to disturb, and only footsteps that land in it are kept.
    pub(crate) fn update_wake(&mut self, camera: Vec3, people: impl IntoIterator<Item = (u32, Vec3)>) {
        let now = self.surface_wetness_last_time;
        let Some(field) = self.cpu_field.as_ref().filter(|_| self.puddle_amount > 10.0 * RAIN_PUDDLE_EPSILON)
        else {
            self.wake.clear();
            return;
        };
        let (accumulation, scatter) = (self.puddle_amount, self.puddle_scatter);
        self.wake.update(now, camera, people, &|position: Vec3| {
            field.puddle_at(position.to_array(), accumulation, scatter)
        });
    }

    /// Splashes made since the last call, for the FX system to play.
    pub(crate) fn take_water_splashes(&mut self) -> Vec<super::wake::WaterSplash> {
        self.wake.take_splashes()
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
            label: Some("JKA precipitation haze exposure"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.haze.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
    }

    pub(crate) fn advance_surface_wetness(&mut self, frame_time: f32, camera_position: [f32; 3]) -> bool {
        let was_active = self.surface_wetness > RAIN_WETNESS_EPSILON
            || self.puddle_amount > RAIN_PUDDLE_EPSILON;
        let dt = if self.surface_wetness_last_time > 0.0 {
            (frame_time - self.surface_wetness_last_time).clamp(0.0, 0.25)
        } else {
            1.0 / 120.0
        };
        self.surface_wetness_last_time = frame_time;

        let exposure_target = self
            .cpu_field
            .as_ref()
            .map_or(1.0, |field| field.exposure_at(camera_position));
        let settle = 1.0 - (-dt / CAMERA_EXPOSURE_SECONDS).exp();
        self.camera_exposure += (exposure_target - self.camera_exposure) * settle;

        let wet_target = self.native_precipitation.unwrap_or(if self.enabled {1.0}else{0.0});
        if wet_target > self.surface_wetness {
            self.surface_wetness = (self.surface_wetness + dt / RAIN_WETNESS_RISE_SECONDS).min(wet_target);
        } else {
            self.surface_wetness = (self.surface_wetness - dt / RAIN_WETNESS_DRY_SECONDS).max(wet_target);
        }

        let puddle_target = if let Some(rain)=self.native_precipitation {rain*rain} else if self.enabled {
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

    pub(crate) fn write_surface_uniform(
        &self,
        queue: &wgpu::Queue,
        buffer: &wgpu::Buffer,
        environment: &WeatherSurfaceEnvironment,
    ) {
        let (occlusion_uv, width, height, active) = match &self.occlusion {
            WeatherOcclusionCache::Ready(info) => (
                [info.min_xz[0], info.min_xz[1], info.inv_extent_xz[0], info.inv_extent_xz[1]],
                info.width, info.height, 1,
            ),
            WeatherOcclusionCache::Pending(_) | WeatherOcclusionCache::Building(_) | WeatherOcclusionCache::Unavailable => ([0.0; 4], 0, 0, 0),
        };
        let intensity = self.intensity_response();
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
                    self.puddle_debug_mode as f32,
                ],
                occlusion_uv,
                occlusion_size: [width, height, active, 0],
                look: {
                    let high = self.puddle_quality.is_high();
                    [
                        self.puddle_scatter,
                        if high { 1.0 } else { 0.0 },
                        if high { 1.0 } else { 0.0 },
                        if high { RAIN_FILM_GLOSS_HIGH } else { 1.0 },
                    ]
                },
                film_fade: [
                    RAIN_FILM_FADE_START,
                    RAIN_FILM_FADE_END,
                    self.sky_blend_window,
                    self.sky_blend_ceiling,
                ],
                sun_direction: [
                    environment.sun_direction[0],
                    environment.sun_direction[1],
                    environment.sun_direction[2],
                    (environment.sun_intensity / 250.0).clamp(0.0, 4.0),
                ],
                sun_radiance: [
                    environment.sun_color[0].max(0.0),
                    environment.sun_color[1].max(0.0),
                    environment.sun_color[2].max(0.0),
                    if environment.has_sky { 1.0 } else { 0.0 },
                ],
                wind: [environment.wind[0], environment.wind[1], 0.0, 0.0],
                wake_info: [
                    environment.wake.len().min(WAKE_MAX_EVENTS) as f32,
                    WAKE_LIFETIME,
                    WAKE_RING_SPEED,
                    self.surface_wetness_last_time,
                ],
                wake_a: std::array::from_fn(|i| {
                    environment.wake.get(i).map_or([0.0; 4], WakeEvent::packed_position)
                }),
                wake_b: std::array::from_fn(|i| {
                    environment.wake.get(i).map_or([0.0; 4], WakeEvent::packed_shape)
                }),
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
            rverbose!(1, "Weather occlusion: no usable static BSP solid footprint");
            return None;
        };
        let basin_cells = built.surface_field.iter().filter(|cell| cell[3] > 0.01).count();
        let scatter_cells = built.surface_field.iter().filter(|cell| cell[2] > 0.05).count();
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
            &built.shelter_level,
        );
        let weather_height_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let empty_water = vec![
            [WEATHER_NO_SURFACE_HEIGHT, WEATHER_NO_SURFACE_HEIGHT, 0.0, 0.0];
            (built.info.width * built.info.height) as usize
        ];
        let (water_height_texture, water_height_view) = create_rain_height_texture(
            device,
            queue,
            "JKA map-wide rain water-height field",
            built.info.width,
            built.info.height,
            &empty_water,
            &[],
        );
        self.gpu.sim_bind_group = create_rain_sim_bind_group(
            device,
            &self.gpu.sim_layout,
            &self.gpu.particle_buffer,
            &self.gpu.sim_uniform_buffer,
            &self.gpu.water_mask_buffer,
            &view,
            &water_height_view,
            &self.gpu.wind_noise_view,
            &self.gpu.wind_noise_sampler,
        );
        self.gpu._collision_texture = texture;
        self.gpu.collision_view = view;
        self.gpu._water_height_texture = water_height_texture;
        self.gpu.water_height_view = water_height_view;
        self.water_field_dirty = true;
        self.cpu_field = Some(CpuField::new(built.info, built.surface_field));
        self.occlusion = WeatherOcclusionCache::Ready(built.info);
        rverbose!(
            1,
            "Weather surface field: background CPU solve {}x{} from {} BSP brush(es), {} blocker triangle(s), {} rendered topography triangle(s); {}/{} top-visible topography texels are puddle basins, {} are large flat ground for scattered puddles; CPU {:.1} ms",
            built.info.width, built.info.height, built.brush_count, built.triangle_count,
            built.topography_triangle_count, basin_cells, topography_cells, scatter_cells, built.build_ms,
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
        weather_wind: crate::ocean::AtmosphericWind,
        sun_color: [f32; 3],
        sun_intensity: f32,
        water_boxes: &[crate::ocean::WaterVolume],
        water_masks: &Arc<crate::ocean::OceanMasks>,
        camera_underwater: bool,
    ) {
        let masks_changed = !Arc::ptr_eq(&self.water_masks, water_masks);
        let volumes_changed = self.water_volumes.as_slice() != water_boxes;
        if masks_changed {
            let mut uploaded = RainWaterMasks::zeroed();
            uploaded.ranges = water_masks.ranges;
            uploaded.bounds = water_masks.bounds;
            uploaded.tiles = water_masks.tiles;
            uploaded.triangles[..water_masks.triangles.len()].copy_from_slice(&water_masks.triangles);
            queue.write_buffer(&self.gpu.water_mask_buffer, 0, bytemuck::bytes_of(&uploaded));
            self.water_masks = Arc::clone(water_masks);
        }
        if masks_changed || volumes_changed {
            self.water_volumes.clear();
            self.water_volumes.extend_from_slice(water_boxes);
            self.water_field_dirty = true;
        }
        if self.water_field_enabled && self.water_field_dirty {
            if let WeatherOcclusionCache::Ready(info) = &self.occlusion {
                let started = Instant::now();
                let field = build_rain_water_height_field(info, water_boxes, water_masks);
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.gpu._water_height_texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    bytemuck::cast_slice(&field),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(info.width * 16),
                        rows_per_image: Some(info.height),
                    },
                    wgpu::Extent3d {
                        width: info.width,
                        height: info.height,
                        depth_or_array_layers: 1,
                    },
                );
                self.water_field_dirty = false;
                rverbose!(
                    1,
                    "Rain water field: baked {}x{} for {} promoted volume(s) in {:.2} ms",
                    info.width,
                    info.height,
                    water_boxes.len(),
                    started.elapsed().as_secs_f64() * 1000.0,
                );
            }
        }
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
        let camera_xz = [camera.position.x, camera.position.z];
        let mut camera_delta = self.camera_anchor_xz.map_or([0.0, 0.0], |previous| {
            [camera_xz[0] - previous[0], camera_xz[1] - previous[1]]
        });
        self.camera_anchor_xz = Some(camera_xz);
        // Teleports should let the normal volume-bounds path reseed at the new
        // location instead of translating a whole old rain field across the map.
        if camera_delta[0] * camera_delta[0] + camera_delta[1] * camera_delta[1]
            > (spawn_radius * 0.5) * (spawn_radius * 0.5)
        {
            camera_delta = [0.0, 0.0];
        }
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
                    camera_delta[0],
                    camera_delta[1],
                ]
            },
            collision_uv,
            counts: [
                self.particle_count(),
                collision_width,
                collision_height,
                self.splash_particle_count(),
            ],
            water_info: [
                water_boxes.len().min(RAIN_MAX_WATER_VOLUMES) as f32,
                if self.water_field_enabled && !self.water_field_dirty { 1.0 } else { 0.0 },
                if self.fast_vertex { 1.0 } else { 0.0 },
                0.0,
            ],
            // The min lanes' spare w carries each volume's outline slot + 1.
            water_min: water_volume_lanes(water_boxes, |v| {
                [v.minimum[0], v.minimum[1], v.minimum[2], v.mask_lane()]
            }),
            water_max: water_volume_lanes(water_boxes, |v| {
                [v.maximum[0], v.maximum[1], v.maximum[2], 0.0]
            }),
        };
        queue.write_buffer(&self.gpu.sim_uniform_buffer, 0, bytemuck::bytes_of(&sim));

        let sun_scale = (sun_intensity / 250.0).max(0.0).sqrt().clamp(0.28, 1.15);
        let tint = Vec3::from_array(self.tint.map(|channel| channel.clamp(0.0, 1.0)));
        let base = if (tint - Vec3::ONE).length_squared() > 0.0001 {
            tint
        } else {
            Vec3::new(0.72, 0.80, 0.90)
        };
        let sun_color = Vec3::from_array(sun_color).max(Vec3::splat(0.05));
        let lit = (base * 0.72 + sun_color * 0.28) * sun_scale;
        let render = RainRenderUniform {
            appearance: [streak_length, streak_width, opacity, splash_size],
            splash: [splash_time, viewport_width.max(1) as f32, viewport_height.max(1) as f32, self.puddle_amount],
            color: [lit.x, lit.y, lit.z, 1.0],
            look: [
                self.puddle_scatter,
                if camera_underwater { 1.0 } else { 0.0 },
                if self.fast_vertex { 1.0 } else { 0.0 },
                0.0,
            ],
        };
        queue.write_buffer(&self.gpu.render_uniform_buffer, 0, bytemuck::bytes_of(&render));
    }
}

fn water_volume_lanes(
    boxes: &[crate::ocean::WaterVolume],
    pick: impl Fn(&crate::ocean::WaterVolume) -> [f32; 4],
) -> [[f32; 4]; RAIN_MAX_WATER_VOLUMES] {
    let mut lanes = [[0.0; 4]; RAIN_MAX_WATER_VOLUMES];
    for (lane, water) in lanes.iter_mut().zip(boxes) {
        *lane = pick(water);
    }
    lanes
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
        source: wgpu::ShaderSource::Wgsl(super::with_post_settings(include_str!("../rain_haze_mask.wgsl")).into()),
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
        cache: crate::pipeline_cache::get().as_ref(),
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
    let (info, surface_field) = super::field::build_heightfield(source)?;
    let shelter_level = super::field::build_shelter_level(&info, &surface_field);
    Some(WeatherOcclusionBuildResult {
        info,
        surface_field,
        shelter_level,
        brush_count,
        triangle_count,
        topography_triangle_count,
        build_ms: started.elapsed().as_secs_f64() * 1000.0,
    })
}

/// Bake promoted water into the weather XZ grid. The hot rain compute path then
/// replaces an 8-volume + outline-triangle search with one textureLoad. This is
/// intentionally CPU work: promoted water changes rarely (normally only map/load
/// or an authoring edit), while rain simulation runs every rendered frame.
fn build_rain_water_height_field(
    info: &WeatherOcclusionInfo,
    water_boxes: &[crate::ocean::WaterVolume],
    water_masks: &crate::ocean::OceanMasks,
) -> Vec<[f32; 4]> {
    let mut field = vec![
        [WEATHER_NO_SURFACE_HEIGHT, WEATHER_NO_SURFACE_HEIGHT, 0.0, 0.0];
        (info.width * info.height) as usize
    ];
    if water_boxes.is_empty() || info.width == 0 || info.height == 0 {
        return field;
    }
    let extent_x = 1.0 / info.inv_extent_xz[0];
    let extent_z = 1.0 / info.inv_extent_xz[1];
    let step_x = extent_x / info.width as f32;
    let step_z = extent_z / info.height as f32;
    let clamp_range = |lo: f32, hi: f32, origin: f32, inv_extent: f32, size: u32| {
        let a = (((lo - origin) * inv_extent) * size as f32).floor() as i64;
        let b = (((hi - origin) * inv_extent) * size as f32).ceil() as i64;
        (a.clamp(0, size as i64) as u32, b.clamp(0, size as i64) as u32)
    };
    for water in water_boxes.iter().take(RAIN_MAX_WATER_VOLUMES) {
        let (x0, x1) = clamp_range(
            water.minimum[0],
            water.maximum[0],
            info.min_xz[0],
            info.inv_extent_xz[0],
            info.width,
        );
        let (z0, z1) = clamp_range(
            water.minimum[2],
            water.maximum[2],
            info.min_xz[1],
            info.inv_extent_xz[1],
            info.height,
        );
        for z in z0..z1 {
            let world_z = info.min_xz[1] + (z as f32 + 0.5) * step_z;
            for x in x0..x1 {
                let world_x = info.min_xz[0] + (x as f32 + 0.5) * step_x;
                if let Some(slot) = water.mask_slot {
                    if !water_masks.contains(usize::from(slot), [world_x, world_z]) {
                        continue;
                    }
                }
                let cell = &mut field[(z * info.width + x) as usize];
                // Rain falls top-down, so the highest overlapping promoted water
                // is the first physically relevant surface at this XZ.
                if water.maximum[1] > cell[1] {
                    *cell = [water.minimum[1], water.maximum[1], 0.0, 0.0];
                }
            }
        }
    }
    field
}

pub(crate) fn create_rain_height_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &'static str,
    width: u32,
    height: u32,
    data: &[[f32; 4]],
    shelter_level: &[[f32; 4]],
) -> (wgpu::Texture, wgpu::TextureView) {
    // Level 1 carries the blurred cover statistics (see field.rs, SHELTER_FOOTPRINT)
    // and is only present when the caller supplies it. Every other reader of this
    // texture loads level 0 and never notices.
    let (half_width, half_height) = ((width / 2).max(1), (height / 2).max(1));
    let with_shelter = shelter_level.len() == (half_width * half_height) as usize && width >= 2 && height >= 2;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: if with_shelter { 2 } else { 1 },
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let upload = |mip_level: u32, texels: &[[f32; 4]], width: u32, height: u32| {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(texels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 16),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    };
    upload(0, data, width, height);
    if with_shelter {
        upload(1, shelter_level, half_width, half_height);
    }
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

pub(crate) fn create_rain_sim_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    particle_buffer: &wgpu::Buffer,
    sim_uniform_buffer: &wgpu::Buffer,
    water_mask_buffer: &wgpu::Buffer,
    collision_view: &wgpu::TextureView,
    water_height_view: &wgpu::TextureView,
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
            wgpu::BindGroupEntry {
                binding: 5,
                resource: water_mask_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(water_height_view),
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
        &[],
    );
    let (water_height_texture, water_height_view) = create_rain_height_texture(
        device,
        queue,
        "JKA rain water-height placeholder",
        1,
        1,
        &[[WEATHER_NO_SURFACE_HEIGHT, WEATHER_NO_SURFACE_HEIGHT, 0.0, 0.0]],
        &[],
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
            water_info: [0.0; 4],
            water_min: [[0.0; 4]; RAIN_MAX_WATER_VOLUMES],
            water_max: [[0.0; 4]; RAIN_MAX_WATER_VOLUMES],
        }),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let water_mask_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA GPU rain water outlines"),
        contents: bytemuck::bytes_of(&RainWaterMasks::zeroed()),
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
            uniform_entry(5, wgpu::ShaderStages::COMPUTE),
            unfilterable_texture_entry_compute(6),
        ],
    });
    let sim_bind_group = create_rain_sim_bind_group(
        device,
        &sim_layout,
        &particle_buffer,
        &sim_uniform_buffer,
        &water_mask_buffer,
        &collision_view,
        &water_height_view,
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
        cache: crate::pipeline_cache::get().as_ref(),
    });

    let render_uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("JKA GPU rain render uniform"),
        contents: bytemuck::bytes_of(&RainRenderUniform {
            appearance: [18.0, 1.40, 0.21, 11.5],
            splash: [0.26, 1.0, 1.0, 0.0],
            color: [0.82, 0.88, 0.94, 1.0],
            look: [0.0; 4],
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
        source: wgpu::ShaderSource::Wgsl(super::with_weather_surface(include_str!("../rain.wgsl")).into()),
    });

    RainGpuResources {
        _collision_texture: collision_texture,
        collision_view,
        _water_height_texture: water_height_texture,
        water_height_view,
        wind_noise_view,
        wind_noise_sampler,
        particle_buffer,
        sim_uniform_buffer,
        water_mask_buffer,
        sim_layout,
        sim_bind_group,
        sim_pipeline,
        render_uniform_buffer,
        render_bind_group,
        render_pipeline_layout,
        render_shader,
        drop_pipeline: None,
        splash_pipeline: None,
        render_format: surface_format,
        render_samples: sample_count,
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
        cache: crate::pipeline_cache::get().as_ref(),
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
        cache: crate::pipeline_cache::get().as_ref(),
    })
}

#[cfg(test)]
pub(crate) mod layout_tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// Byte size and member offsets of a named WGSL struct, as naga lays it out.
    pub(crate) fn wgsl_struct_layout(source: &str, name: &str) -> (u32, Vec<(String, u32)>) {
        let module = wgpu::naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
        let (_, ty) = module
            .types
            .iter()
            .find(|(_, ty)| ty.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("WGSL struct {name} not found"));
        let wgpu::naga::TypeInner::Struct { members, span } = &ty.inner else {
            panic!("{name} is not a struct");
        };
        let offsets = members
            .iter()
            .map(|member| (member.name.clone().unwrap_or_default(), member.offset))
            .collect();
        (*span, offsets)
    }

    pub(crate) fn offset_of_member(members: &[(String, u32)], name: &str) -> u32 {
        members
            .iter()
            .find(|(member, _)| member == name)
            .unwrap_or_else(|| panic!("WGSL member {name} not found"))
            .1
    }

    #[test]
    fn weather_surface_uniform_matches_its_wgsl_struct() {
        let (size, members) = wgsl_struct_layout(
            include_str!("../weather_surface.wgsl"),
            "WeatherSurfaceSettings",
        );
        assert_eq!(size as usize, size_of::<WeatherSurfaceUniform>());
        for (name, rust_offset) in [
            ("amount_distance", offset_of!(WeatherSurfaceUniform, amount_distance)),
            ("puddle", offset_of!(WeatherSurfaceUniform, puddle)),
            ("occlusion_uv", offset_of!(WeatherSurfaceUniform, occlusion_uv)),
            ("occlusion_size", offset_of!(WeatherSurfaceUniform, occlusion_size)),
            ("look", offset_of!(WeatherSurfaceUniform, look)),
            ("film_fade", offset_of!(WeatherSurfaceUniform, film_fade)),
            ("sun_direction", offset_of!(WeatherSurfaceUniform, sun_direction)),
            ("sun_radiance", offset_of!(WeatherSurfaceUniform, sun_radiance)),
            ("wind", offset_of!(WeatherSurfaceUniform, wind)),
            ("wake_info", offset_of!(WeatherSurfaceUniform, wake_info)),
            ("wake_a", offset_of!(WeatherSurfaceUniform, wake_a)),
            ("wake_b", offset_of!(WeatherSurfaceUniform, wake_b)),
        ] {
            assert_eq!(offset_of_member(&members, name) as usize, rust_offset, "{name}");
        }
    }

    #[test]
    fn rain_render_uniform_matches_its_wgsl_struct() {
        let source = super::super::with_weather_surface(include_str!("../rain.wgsl"));
        let (size, members) = wgsl_struct_layout(&source, "RainRenderUniform");
        assert_eq!(size as usize, size_of::<RainRenderUniform>());
        for (name, rust_offset) in [
            ("appearance", offset_of!(RainRenderUniform, appearance)),
            ("splash", offset_of!(RainRenderUniform, splash)),
            ("color", offset_of!(RainRenderUniform, color)),
            ("look", offset_of!(RainRenderUniform, look)),
        ] {
            assert_eq!(offset_of_member(&members, name) as usize, rust_offset, "{name}");
        }
    }

    #[test]
    fn rain_sim_uniform_matches_its_wgsl_struct() {
        let source = include_str!("../rain_sim.wgsl");
        let (size, _) = wgsl_struct_layout(source, "RainSimUniform");
        assert_eq!(size as usize, size_of::<RainSimUniform>());
        let (particle, _) = wgsl_struct_layout(source, "RainParticle");
        assert_eq!(particle as usize, size_of::<RainParticle>());
        let (masks, members) = wgsl_struct_layout(source, "WaterMasks");
        assert_eq!(masks as usize, size_of::<RainWaterMasks>());
        for (name, rust_offset) in [
            ("ranges", offset_of!(RainWaterMasks, ranges)),
            ("bounds", offset_of!(RainWaterMasks, bounds)),
            ("tiles", offset_of!(RainWaterMasks, tiles)),
            ("triangles", offset_of!(RainWaterMasks, triangles)),
        ] {
            assert_eq!(offset_of_member(&members, name) as usize, rust_offset, "{name}");
        }
    }

    #[test]
    fn rain_glass_wind_is_projected_into_lens_space() {
        let camera = Camera::new_with_fov([0.0; 3], 0.0, 80.0);
        let crosswind = crate::ocean::AtmosphericWind {
            speed: 180.0,
            direction: 90.0,
            gust: 0.0,
            shift: 0.0,
        };
        let cross = rain_glass_wind_lens(&camera, crosswind, 0.0);
        assert!((cross[0] - 180.0).abs() < 0.001);
        assert!(cross[1].abs() < 0.001);
        assert!(cross[2].abs() < 0.001);

        let headwind = crate::ocean::AtmosphericWind {
            speed: 180.0,
            direction: 180.0,
            gust: 0.0,
            shift: 0.0,
        };
        let head = rain_glass_wind_lens(&camera, headwind, 0.0);
        assert!(head[0].abs() < 0.001);
        assert!(head[1].abs() < 0.001);
        assert!((head[2] - 180.0).abs() < 0.001);
    }
}

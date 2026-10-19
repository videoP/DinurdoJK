//! Rain near the camera, splashes on the sea, lightning and the thunder cue, ported from
//! the web game's `sky/weather/WeatherSystem.ts` with `rain.ts`, `splashes.ts` and `boltMesh.ts` (the CPU side of each) and
//! their shaders (`shaders/rain.wgsl`, `splashes.wgsl`, `bolt.wgsl`). The rain veil is a post pass (`crate::post`).

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use naval_render::{
    MakePipelines, MotionPipeline, Pipeline, PipelineId, PipelineRegistry, PlaceholderPipelines,
    RenderContext, ShaderError, UploadSink, VertexLayout,
};
use naval_sky::{BoltLight, SkyQuality, SkyScene};
use wgpu::util::DeviceExt;

use crate::bolt::{self, MAX_BOLT_SEGMENTS};
use crate::gpu;
use crate::layer::CloudView;
use crate::lightning::{
    self, LightningScheduler, Random, Strike, StrikeClouds, StrikeKind, Thunder,
};
use crate::quality::{self, MOST_RAIN_DROPS};
use crate::smoothstep;
use crate::{Targets, shaders};

/// Drops live in four nested boxes: edge (m), share of the drops, terminal speed (m/s) of the class's largest drops.
const RAIN_CLASSES: [(f32, f32, f32); 4] = [
    (6.0, 0.14, 8.8),
    (14.0, 0.36, 9.3),
    (30.0, 0.34, 8.4),
    (64.0, 0.16, 9.6),
];
const FORWARD_SHIFT: f32 = 0.8;
const DIAMETER_HEAVY: f32 = 0.0034;
const DIAMETER_LIGHT: f32 = 0.0012;
const HIGH: [f64; 2] = [40.0, 300.0];
const SURFACE_WIND: f64 = 0.75;
const VELOCITY_SMOOTHING: f64 = 0.12;
const MAX_CAMERA_SPEED: f32 = 250.0;
const RAIN_CEILING: [f64; 2] = [1000.0, 1500.0];
const SPLASH_CEILING: [f64; 2] = [45.0, 120.0];
const REFERENCE_DROPS: f64 = 8000.0;
const SPLASH_SHARE: f64 = 0.15;
const SPLASH_ZOOM: f32 = 3.0;
const SUBMERGED: f32 = -0.3;
const BOLT_VISIBILITY_DRY: f64 = 40000.0;
const BOLT_VISIBILITY_RAIN: f64 = 11000.0;
const BOLT_FOOT: f64 = -3.0;
const BOLT_RADIANCE: f64 = 150.0;
const SPLASH_REGION: f32 = 64.0;
const SPLASH_FAR: f32 = 100.0;
const SPLASH_PERIODS: [f64; 4] = [0.5, 0.6, 0.75, 1.0];
const SPLASH_CLOCK: f64 = 60.0;

fn direct_share(kind: StrikeKind) -> f64 {
    match kind {
        StrikeKind::Ground => 1.0,
        StrikeKind::Cloud => 0.2,
    }
}

fn wrap(value: f32) -> f32 {
    value - value.floor()
}

fn splash_region(position: Vec3, forward: Vec3) -> [f32; 4] {
    [
        wrap(-(position.x + forward.x) / SPLASH_REGION + 0.5),
        wrap(-(position.z + forward.z) / SPLASH_REGION + 0.5),
        forward.x,
        forward.z,
    ]
}

#[cfg(test)]
mod splash_tests {
    use super::*;

    #[test]
    fn live_impacts_stay_in_world_space_when_aim_or_position_changes() {
        // Reconstruct one live shader instance while its life/seed are held.
        // Regions may wrap at their edges; world positions must be identical
        // modulo that region, including while the forward offset rotates.
        let impact = |eye: Vec3, ahead: Vec3| {
            let region = splash_region(eye, ahead);
            Vec3::new(
                eye.x + region[2] + (wrap(0.21 + region[0]) - 0.5) * SPLASH_REGION,
                0.0,
                eye.z + region[3] + (wrap(0.43 + region[1]) - 0.5) * SPLASH_REGION,
            )
        };
        let first = impact(Vec3::new(113.0, 2.0, -47.0), Vec3::Z * 28.0);
        for (eye, ahead) in [
            (Vec3::new(113.0, 2.0, -47.0), Vec3::X * 28.0),
            (Vec3::new(119.0, 2.0, -39.0), Vec3::NEG_Z * 28.0),
        ] {
            let delta = (impact(eye, ahead) - first) / SPLASH_REGION;
            assert!((delta - delta.round()).length() < 1e-5);
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct RainUniforms {
    boxes: [[f32; 4]; 12],
    view: [[f32; 4]; 4],
    projection: [[f32; 4]; 4],
    camera_velocity: [f32; 3],
    opacity: f32,
    wind: [f32; 3],
    density: f32,
    bolt_direction: [f32; 3],
    bolt_irradiance: f32,
    screen: [f32; 2],
    diameter: f32,
    direct_share: f32,
    grey: f32,
    height: f32,
    camera_altitude: f32,
    padding: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct SplashUniforms {
    view: [[f32; 4]; 4],
    projection: [[f32; 4]; 4],
    cycles: [f32; 4],
    region: [f32; 4],
    camera_xz: [f32; 2],
    camera_height: f32,
    opacity: f32,
    size: f32,
    direct_share: f32,
    grey: f32,
    /// The span (m) of the sea's heights about the camera the splashes stand on (`Weather::set_sea_heights`), 0 for
    /// the flat mean sea.
    sea_span: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct BoltUniforms {
    view: [[f32; 4]; 4],
    projection: [[f32; 4]; 4],
    origin: [f32; 3],
    main: f32,
    screen: [f32; 2],
    branches: f32,
    padding: f32,
}

/// What the weather lays on the rest of the scene this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WeatherLight {
    /// The strike lighting the clouds: channel position and radiant intensity (0 dark).
    pub position: Vec3,
    pub intensity: f32,
    /// The scene flash, a multiple of the ambient.
    pub flash: f32,
    /// The flash's direct light.
    pub bolt: BoltLight,
    /// The strike's irradiance at the camera and the direction toward it (for the rain veil's glow).
    pub irradiance: f32,
    pub direction: Vec3,
}

/// The rain veil's inputs this frame (`WeatherSystem.update`'s haze uniforms).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HazeState {
    pub strength: f32,
    pub height: f32,
    pub ceiling: f32,
    pub grey: f32,
}

/// Rain, splashes, lightning and thunder. See the module docs.
pub struct Weather {
    tier: quality::CloudTier,
    scheduler: LightningScheduler,
    active: Option<Strike>,
    bolt_visible: bool,
    channel_count: u32,
    precipitation: f64,
    clouds: StrikeClouds,
    // Rain state.
    drift: [Vec3; 4],
    velocity: Vec3,
    last_position: Option<Vec3>,
    fall_scale: f32,
    /// Host speed control; independent of precipitation and storm time.
    fall_multiplier: f32,
    wind: Vec3,
    rain_uniforms: RainUniforms,
    rain_count: u32,
    // Splash state.
    clock: f64,
    splash_uniforms: SplashUniforms,
    splash_count: u32,
    bolt_uniforms: BoltUniforms,
    light: WeatherLight,
    haze: HazeState,
    /// Thunder due this frame for the game's audio (`onThunder`).
    pub thunder: Vec<Thunder>,
    // GPU.
    rain_buffer: wgpu::Buffer,
    splash_buffer: wgpu::Buffer,
    bolt_buffer: wgpu::Buffer,
    drops: wgpu::Buffer,
    splashes: wgpu::Buffer,
    bolt_start: wgpu::Buffer,
    bolt_end: wgpu::Buffer,
    rain_group: wgpu::BindGroup,
    splash_group: wgpu::BindGroup,
    bolt_group: wgpu::BindGroup,
    /// The draws' group layout, and the sea's heights about the camera with their repeating sampler (a flat placeholder
    /// until [`Weather::set_sea_heights`]).
    layout: wgpu::BindGroupLayout,
    sea_heights: (wgpu::TextureView, wgpu::Sampler),
    rain_pipeline: wgpu::RenderPipeline,
    splash_pipeline: wgpu::RenderPipeline,
    bolt_pipeline: wgpu::RenderPipeline,
    /// Group 1 (the sky's) and group 2 (the draw's uniforms) of the three pipelines, for the registry's recipes.
    layouts: [wgpu::BindGroupLayout; 2],
    targets: Targets,
}

/// The weather's pipelines on the renderer's registry ([`Weather::register_pipelines`]), taken in by
/// [`Weather::adopt_pipelines`].
#[derive(Clone, Copy, Debug)]
pub struct WeatherPipelines {
    draws: [PipelineId; 3],
    /// The rain's streaks in temporal AA's motion pass ([`Weather::draw_motion`]).
    pub rain_motion: PipelineId,
}

/// The weather's three draws.
#[derive(Clone, Copy, Debug)]
enum Draw {
    Rain,
    Splashes,
    Bolt,
}

const PREMULTIPLIED: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    },
};
const ADDITIVE: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};
const DROP: [wgpu::VertexAttribute; 1] = [wgpu::VertexAttribute {
    format: wgpu::VertexFormat::Float32x4,
    offset: 0,
    shader_location: 0,
}];
const START: [wgpu::VertexAttribute; 1] = DROP;
const END: [wgpu::VertexAttribute; 1] = [wgpu::VertexAttribute {
    format: wgpu::VertexFormat::Float32x4,
    offset: 0,
    shader_location: 1,
}];

/// A draw's group: its uniforms and the sea's heights about the camera.
fn weather_group(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
    (heights, sampler): &(wgpu::TextureView, wgpu::Sampler),
) -> wgpu::BindGroup {
    gpu::bind_group(
        device,
        label,
        layout,
        &[
            (0, buffer.as_entire_binding()),
            (1, wgpu::BindingResource::TextureView(heights)),
            (2, wgpu::BindingResource::Sampler(sampler)),
        ],
    )
}

fn repeating_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("weather sea heights"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    })
}

/// The flat mean sea: a 1×1 texture of zeroes.
fn flat_sea(device: &wgpu::Device) -> (wgpu::TextureView, wgpu::Sampler) {
    let view = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("weather flat sea"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());
    (view, repeating_sampler(device))
}

fn instance_layout(
    attributes: &'static [wgpu::VertexAttribute],
) -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: 16,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes,
    }
}

impl Draw {
    const ALL: [Draw; 3] = [Draw::Rain, Draw::Splashes, Draw::Bolt];

    fn label(self) -> &'static str {
        match self {
            Draw::Rain => "rain",
            Draw::Splashes => "rain splashes",
            Draw::Bolt => "lightning bolt",
        }
    }

    /// The WGSL source and the library path it is registered under.
    fn shader(self) -> (&'static str, &'static str) {
        match self {
            Draw::Rain => (shaders::RAIN_WGSL, shaders::RAIN_PATH),
            Draw::Splashes => (shaders::SPLASHES_WGSL, shaders::SPLASHES_PATH),
            Draw::Bolt => (shaders::BOLT_WGSL, shaders::BOLT_PATH),
        }
    }

    fn pipeline(
        self,
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        [sky_layout, layout]: &[wgpu::BindGroupLayout; 2],
        targets: Targets,
        cache: Option<&wgpu::PipelineCache>,
    ) -> wgpu::RenderPipeline {
        let layouts = [None, Some(sky_layout), Some(layout)];
        let (vertex, fragment, buffers, blend) = match self {
            Draw::Rain => (
                "rain_vertex",
                "rain_fragment",
                vec![Some(instance_layout(&DROP))],
                PREMULTIPLIED,
            ),
            Draw::Splashes => (
                "splash_vertex",
                "splash_fragment",
                vec![Some(instance_layout(&DROP))],
                PREMULTIPLIED,
            ),
            Draw::Bolt => (
                "bolt_vertex",
                "bolt_fragment",
                vec![Some(instance_layout(&START)), Some(instance_layout(&END))],
                ADDITIVE,
            ),
        };
        gpu::scene_pipeline(
            device,
            self.label(),
            module,
            vertex,
            fragment,
            &layouts,
            &buffers,
            Some(blend),
            targets,
            cache,
        )
    }
}

impl Weather {
    pub fn new(
        device: &wgpu::Device,
        sky_layout: &wgpu::BindGroupLayout,
        quality: SkyQuality,
        targets: Targets,
    ) -> Self {
        Self::build(device, sky_layout, quality, targets, MakePipelines::Now)
    }

    /// The same with `placeholders` for its pipelines until [`Weather::adopt_pipelines`] takes the registry's.
    pub fn new_deferred(
        device: &wgpu::Device,
        sky_layout: &wgpu::BindGroupLayout,
        quality: SkyQuality,
        targets: Targets,
        placeholders: &PlaceholderPipelines,
    ) -> Self {
        Self::build(
            device,
            sky_layout,
            quality,
            targets,
            MakePipelines::Deferred(placeholders),
        )
    }

    /// Builds the three draws' pipelines through the renderer's registry, as [`Weather::new`] builds them. The
    /// clouds' files must be registered first (`crate::shaders::register`).
    pub fn register_pipelines(
        &self,
        ctx: &mut RenderContext<'_>,
    ) -> Result<WeatherPipelines, ShaderError> {
        let mut draws = Vec::new();
        for draw in Draw::ALL {
            let layouts = self.layouts.clone();
            let targets = self.targets;
            draws.push(ctx.add_pipeline(
                draw.label(),
                draw.shader().1,
                Vec::new(),
                Box::new(move |build| {
                    Pipeline::Render(draw.pipeline(build.device, build.module, &layouts, targets, build.cache))
                }),
            )?);
        }
        let rain_motion = ctx.add_motion_pipeline(
            "rain motion",
            Draw::Rain.shader().1,
            MotionPipeline {
                vertex_buffers: vec![VertexLayout {
                    stride: 16,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: DROP.to_vec(),
                }],
                bind_group_layouts: self.layouts.to_vec(),
                primitive: wgpu::PrimitiveState::default(),
                ..MotionPipeline::new("rain_vertex", "rain_motion")
            },
        )?;
        Ok(WeatherPipelines {
            draws: [draws[0], draws[1], draws[2]],
            rain_motion,
        })
    }

    /// Takes the registry's current pipelines in place of the weather's own; whether any changed.
    pub fn adopt_pipelines(&mut self, registry: &PipelineRegistry, ids: &WeatherPipelines) -> bool {
        let mut changed = false;
        for (slot, id) in [
            &mut self.rain_pipeline,
            &mut self.splash_pipeline,
            &mut self.bolt_pipeline,
        ]
        .into_iter()
        .zip(ids.draws)
        {
            let new = registry.render(id);
            if slot != new {
                *slot = new.clone();
                changed = true;
            }
        }
        changed
    }

    fn build(
        device: &wgpu::Device,
        sky_layout: &wgpu::BindGroupLayout,
        quality: SkyQuality,
        targets: Targets,
        make: MakePipelines<'_>,
    ) -> Self {
        let uniform = |label: &str, size: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let rain_buffer = uniform("rain", std::mem::size_of::<RainUniforms>());
        let splash_buffer = uniform("splashes", std::mem::size_of::<SplashUniforms>());
        let bolt_buffer = uniform("bolt", std::mem::size_of::<BoltUniforms>());
        // Every drop's seed: its place in the box and its class with a fraction for its own size and brightness.
        let mut random = Random::new(7331);
        let total: f32 = RAIN_CLASSES.iter().map(|c| c.1).sum();
        let mut cumulative = [0f32; 4];
        let mut sum = 0.0;
        for (i, class) in RAIN_CLASSES.iter().enumerate() {
            sum += class.1;
            cumulative[i] = sum;
        }
        let mut drops = Vec::with_capacity(MOST_RAIN_DROPS as usize);
        for _ in 0..MOST_RAIN_DROPS {
            let pick = random.draw() as f32 * total;
            let k = cumulative.iter().position(|&edge| pick < edge).unwrap_or(0);
            drops.push([
                random.draw() as f32,
                random.draw() as f32,
                random.draw() as f32,
                k as f32 + random.draw() as f32 * 0.999,
            ]);
        }
        let capacity = (f64::from(MOST_RAIN_DROPS) * SPLASH_SHARE).ceil() as usize;
        let mut random = Random::new(4217);
        let splashes: Vec<[f32; 4]> = (0..capacity)
            .map(|_| {
                let (x, y, z) = (
                    random.draw() as f32,
                    random.draw() as f32,
                    random.draw() as f32,
                );
                [
                    x,
                    y,
                    z,
                    (random.draw() * 4.0).floor() as f32 + random.draw() as f32 * 0.999,
                ]
            })
            .collect();
        let instance = |label: &str, data: &[[f32; 4]]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(data),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            })
        };
        let drops = instance("rain drops", &drops);
        let splash_instances = instance("rain splashes", &splashes);
        let segments = vec![[0f32; 4]; MAX_BOLT_SEGMENTS];
        let bolt_start = instance("bolt starts", &segments);
        let bolt_end = instance("bolt ends", &segments);
        let layout = gpu::layout(
            device,
            "weather",
            &[
                gpu::uniform_entry(0),
                gpu::texture_2d_entry(1),
                gpu::sampler_entry(2),
            ],
        );
        let sea_heights = flat_sea(device);
        let group = |label: &str, buffer: &wgpu::Buffer| {
            weather_group(device, label, &layout, buffer, &sea_heights)
        };
        let rain_group = group("rain", &rain_buffer);
        let splash_group = group("splashes", &splash_buffer);
        let bolt_group = group("bolt", &bolt_buffer);
        let layouts = [sky_layout.clone(), layout.clone()];
        let [rain_pipeline, splash_pipeline, bolt_pipeline] = Draw::ALL.map(|draw| {
            make.render(|| {
                let (source, path) = draw.shader();
                let module = shaders::shader_module(device, source, path, &[]);
                draw.pipeline(device, &module, &layouts, targets, None)
            })
        });
        let mut weather = Self {
            tier: quality::tier(quality),
            scheduler: LightningScheduler::new(1941),
            active: None,
            bolt_visible: false,
            channel_count: 0,
            precipitation: 0.0,
            clouds: StrikeClouds {
                altitude: 650.0,
                thickness: 3000.0,
            },
            drift: [Vec3::ZERO; 4],
            velocity: Vec3::ZERO,
            last_position: None,
            fall_scale: 1.0,
            fall_multiplier: 1.0,
            wind: Vec3::ZERO,
            rain_uniforms: RainUniforms::default(),
            rain_count: 0,
            clock: 0.0,
            splash_uniforms: SplashUniforms::default(),
            splash_count: 0,
            bolt_uniforms: BoltUniforms::default(),
            light: WeatherLight::default(),
            haze: HazeState::default(),
            thunder: Vec::new(),
            rain_buffer,
            splash_buffer,
            bolt_buffer,
            drops,
            splashes: splash_instances,
            bolt_start,
            bolt_end,
            rain_group,
            splash_group,
            bolt_group,
            layout,
            sea_heights,
            rain_pipeline,
            splash_pipeline,
            bolt_pipeline,
            layouts,
            targets,
        };
        weather.set_quality(quality);
        weather
    }

    /// Stand the splashes on the drawn sea: `heights` the ocean's heights about the camera (an `Rgba16Float` texture,
    /// metres in red, laid on the world in a square of `span` metres that repeats, `naval_ocean::rain_heights`), or the
    /// flat mean sea with `None`.
    pub fn set_sea_heights(
        &mut self,
        device: &wgpu::Device,
        heights: Option<(&wgpu::TextureView, f32)>,
    ) {
        self.sea_heights = match heights {
            Some((view, _)) => (view.clone(), repeating_sampler(device)),
            None => flat_sea(device),
        };
        self.splash_uniforms.sea_span = heights.map_or(0.0, |(_, span)| span);
        self.rain_group = weather_group(
            device,
            "rain",
            &self.layout,
            &self.rain_buffer,
            &self.sea_heights,
        );
        self.splash_group = weather_group(
            device,
            "splashes",
            &self.layout,
            &self.splash_buffer,
            &self.sea_heights,
        );
        self.bolt_group = weather_group(
            device,
            "bolt",
            &self.layout,
            &self.bolt_buffer,
            &self.sea_heights,
        );
    }

    /// Publish a sea binding change made after this frame's weather update.
    pub fn upload_splash_surface(&self, queue: &wgpu::Queue) {
        queue.write_buffer(&self.splash_buffer, 0, bytemuck::bytes_of(&self.splash_uniforms));
    }

    pub fn set_quality(&mut self, quality: SkyQuality) {
        self.tier = quality::tier(quality);
        self.rain_uniforms.density =
            (REFERENCE_DROPS / f64::from(self.tier.rain_drops)).sqrt() as f32;
    }

    /// Scale only vertical drop velocity; 1 preserves the upstream terminal speeds.
    pub fn set_fall_speed(&mut self, multiplier: f32) {
        self.fall_multiplier = if multiplier.is_finite() { multiplier.clamp(0.25, 4.0) } else { 1.0 };
    }

    /// Apply a scene's weather (`WeatherSystem.apply`).
    pub fn apply(&mut self, scene: &SkyScene) {
        let (weather, clouds) = (&scene.weather, &scene.clouds);
        self.precipitation = weather.precipitation.clamp(0.0, 1.0);
        self.scheduler.rate = weather.lightning.max(0.0);
        self.clouds = StrikeClouds {
            altitude: clouds.altitude,
            thickness: clouds.thickness,
        };
        // Rain's look for a precipitation: drizzle is finer and falls slower than a downpour.
        let heavy = smoothstep(0.1, 0.8, self.precipitation) as f32;
        self.rain_uniforms.diameter = DIAMETER_LIGHT + (DIAMETER_HEAVY - DIAMETER_LIGHT) * heavy;
        self.fall_scale = 0.55 + 0.45 * heavy;
        let heading = clouds.wind_heading.to_radians();
        self.wind = Vec3::new(heading.sin() as f32, 0.0, heading.cos() as f32)
            * (clouds.wind_speed * SURFACE_WIND) as f32;
        // The deck hides the sun from the drops and greys the light filling them.
        let direct = (1.0 - smoothstep(0.55, 0.95, clouds.coverage)) as f32;
        let grey = smoothstep(0.3, 0.9, clouds.coverage) as f32;
        self.rain_uniforms.direct_share = direct;
        self.splash_uniforms.direct_share = direct;
        self.rain_uniforms.grey = grey;
        self.splash_uniforms.grey = grey;
        self.haze.grey = grey;
        self.splash_uniforms.size = (0.5 + 0.5 * self.precipitation.sqrt()) as f32;
    }

    /// Diagnostics (`forceStrike`): fire a flash of `kind` at `position` (where a ground bolt meets the sea) and show
    /// it `age` seconds after its first return stroke, its flicker and bolt from `seed`. No thunder is cued; frames
    /// with `dt` 0 hold it.
    pub fn force_strike(
        &mut self,
        queue: &wgpu::Queue,
        position: Vec3,
        age: f64,
        kind: StrikeKind,
        seed: u32,
    ) {
        let mut random = Random::new(seed);
        let strokes = lightning::plan_strokes(kind, &mut random);
        let strike_seed = (random.draw() * 4_294_967_296.0).floor() as u32;
        let mut strike = Strike::new(kind, strokes, strike_seed);
        strike.ground = Vec3::new(position.x, 0.0, position.z);
        lightning::place_channel(&mut strike, self.clouds, &mut random);
        strike.age = age.max(0.0);
        strike.expose(strike.age, strike.age);
        self.show(&mut UploadSink::Queue(queue), strike, false);
    }

    /// Diagnostics: end any flash now.
    pub fn clear_strike(&mut self) {
        self.active = None;
        self.bolt_visible = false;
        self.light = WeatherLight::default();
    }

    pub fn strike(&self) -> Option<&Strike> {
        self.active.as_ref()
    }

    /// Lightning's light on the rest of the scene this frame.
    pub fn light(&self) -> WeatherLight {
        self.light
    }

    pub fn haze(&self) -> HazeState {
        self.haze
    }

    /// Advance the weather by `dt` for `view` (`WeatherSystem.update`); `sheltered` keeps rain out of hull views. Its
    /// writes go through `queue` (the renderer core's staging belt in a layer's `prepare`).
    pub fn update(
        &mut self,
        queue: &mut UploadSink<'_>,
        view: &CloudView,
        dt: f64,
        cut: bool,
        sheltered: bool,
    ) {
        self.thunder.clear();
        let position = view.position;
        self.update_lightning(queue, position, dt);
        let altitude = f64::from(position.y);
        let indoors = position.y < SUBMERGED || sheltered;
        let aloft = 1.0 - smoothstep(RAIN_CEILING[0], RAIN_CEILING[1], altitude);
        let rain = if indoors {
            0.0
        } else {
            self.precipitation * aloft
        };
        self.update_rain(view, dt, cut);
        // Light rain is many faint streaks: count and opacity both follow the square root of the precipitation.
        let share = self.precipitation.sqrt();
        self.rain_uniforms.opacity = (aloft * share) as f32;
        self.rain_count = if rain > 0.0 {
            (share * f64::from(self.tier.rain_drops)).round() as u32
        } else {
            0
        }
        .min(MOST_RAIN_DROPS);
        let magnification = view.projection.y_axis.y * 26f32.to_radians().tan();
        let splash = if indoors || magnification > SPLASH_ZOOM {
            0.0
        } else {
            rain * (1.0 - smoothstep(SPLASH_CEILING[0], SPLASH_CEILING[1], altitude))
        };
        self.update_splashes(view, dt);
        self.splash_uniforms.opacity = (splash / self.precipitation.max(1e-3)).min(1.0) as f32;
        let capacity = (f64::from(MOST_RAIN_DROPS) * SPLASH_SHARE).ceil() as u32;
        self.splash_count = if splash > 0.0 {
            (self.precipitation * f64::from(self.tier.rain_drops) * SPLASH_SHARE).round() as u32
        } else {
            0
        }
        .min(capacity);
        self.haze.strength = rain as f32;
        self.haze.height = position.y.max(0.5);
        self.haze.ceiling = (self.clouds.altitude as f32 - position.y).max(0.0);
        // The drawing buffer and camera the meshes project with.
        let screen = [view.viewport[0] as f32, view.viewport[1] as f32];
        let view_matrix = view.view.to_cols_array_2d();
        let projection = view.projection.to_cols_array_2d();
        self.rain_uniforms.view = view_matrix;
        self.rain_uniforms.projection = projection;
        self.rain_uniforms.screen = screen;
        self.rain_uniforms.camera_altitude = position.y;
        self.rain_uniforms.bolt_irradiance = self.light.irradiance;
        self.rain_uniforms.bolt_direction = self.light.direction.to_array();
        self.splash_uniforms.view = view_matrix;
        self.splash_uniforms.projection = projection;
        self.bolt_uniforms.view = view_matrix;
        self.bolt_uniforms.projection = projection;
        self.bolt_uniforms.screen = screen;
        queue.write_buffer(
            &self.rain_buffer,
            0,
            bytemuck::bytes_of(&self.rain_uniforms),
        );
        queue.write_buffer(
            &self.splash_buffer,
            0,
            bytemuck::bytes_of(&self.splash_uniforms),
        );
        queue.write_buffer(
            &self.bolt_buffer,
            0,
            bytemuck::bytes_of(&self.bolt_uniforms),
        );
    }

    fn update_rain(&mut self, view: &CloudView, dt: f64, cut: bool) {
        let position = view.position;
        match self.last_position {
            Some(last) if !cut && dt > 0.0 => {
                let mut raw = (position - last) / dt as f32;
                if raw.length() > MAX_CAMERA_SPEED {
                    raw = raw.normalize() * MAX_CAMERA_SPEED;
                }
                self.velocity = self
                    .velocity
                    .lerp(raw, (1.0 - (-dt / VELOCITY_SMOOTHING).exp()) as f32);
            }
            Some(_) if !cut => {}
            _ => self.velocity = Vec3::ZERO,
        }
        self.last_position = Some(position);
        self.rain_uniforms.camera_velocity = self.velocity.to_array();
        self.rain_uniforms.height = smoothstep(HIGH[0], HIGH[1], f64::from(position.y)) as f32;
        // Zoomed optics look past the near drops: every box moves out along the view by the magnification.
        let magnification = (view.projection.y_axis.y * 26f32.to_radians().tan()).max(1.0);
        let forward = view
            .view
            .inverse()
            .transform_vector3(Vec3::NEG_Z)
            .normalize();
        let wind = self.wind;
        for (k, &(size, _, fall)) in RAIN_CLASSES.iter().enumerate() {
            let speed = fall * self.fall_scale * self.fall_multiplier;
            let drift = &mut self.drift[k];
            if dt > 0.0 {
                let dt = dt as f32;
                *drift = Vec3::new(
                    wrap(drift.x + wind.x * dt / size),
                    (drift.y + speed * dt / size) % 10.0,
                    wrap(drift.z + wind.z * dt / size),
                );
            }
            let offset = forward * (FORWARD_SHIFT * size / 2.0 * magnification);
            let x = wrap(drift.x - (position.x + offset.x) / size + 0.5);
            let y = wrap(0.5 - (position.y + offset.y) / size);
            let z = wrap(drift.z - (position.z + offset.z) / size + 0.5);
            self.rain_uniforms.boxes[k * 3] = [x, y, z, size];
            self.rain_uniforms.boxes[k * 3 + 1] = [offset.x, offset.y, offset.z, speed];
            self.rain_uniforms.boxes[k * 3 + 2] = [drift.y, 0.0, 0.0, 0.0];
        }
        self.rain_uniforms.wind = wind.to_array();
    }

    fn update_splashes(&mut self, view: &CloudView, dt: f64) {
        if dt > 0.0 {
            self.clock = (self.clock + dt) % SPLASH_CLOCK;
        }
        let u = &mut self.splash_uniforms;
        u.cycles = SPLASH_PERIODS.map(|period| (self.clock / period) as f32);
        let position = view.position;
        let mut forward = view.view.inverse().transform_vector3(Vec3::NEG_Z);
        forward.y = 0.0;
        if forward.length_squared() < 1e-6 {
            forward = Vec3::Z;
        }
        // A level camera sees the sea from height / tan(half the vertical field) ahead.
        let nearest = position.y.max(0.0) * view.projection.y_axis.y;
        let forward = forward.normalize()
            * SPLASH_FAR.min((SPLASH_REGION * 0.3).max(nearest * 0.8 + SPLASH_REGION * 0.4));
        // Cancel the moving region centre: a live impact keeps its world
        // position when the camera translates or changes its aim.
        u.region = splash_region(position, forward);
        u.camera_height = position.y;
        u.camera_xz = [position.x, position.z];
    }

    fn update_lightning(&mut self, queue: &mut UploadSink<'_>, camera: Vec3, dt: f64) {
        let due = self.scheduler.advance(dt);
        let mut fresh = false;
        for _ in 0..due {
            let strike = self.scheduler.next(camera, self.clouds);
            self.show(queue, strike, true);
            fresh = true;
        }
        // A strike shows its first stroke's peak the frame it fires, then each frame's exposure.
        if fresh {
            if let Some(active) = &mut self.active {
                active.expose(0.0, 0.0);
            }
        } else if let Some(active) = &mut self.active {
            active.advance(dt);
        }
        if self.active.as_ref().is_some_and(Strike::done) {
            self.active = None;
        }
        self.bolt_visible = false;
        let Some(active) = &self.active else {
            self.light = WeatherLight::default();
            self.bolt_uniforms.main = 0.0;
            self.bolt_uniforms.branches = 0.0;
            return;
        };
        let to_strike = active.position - camera;
        let distance = to_strike.length().max(1.0);
        let air =
            BOLT_VISIBILITY_DRY + (BOLT_VISIBILITY_RAIN - BOLT_VISIBILITY_DRY) * self.precipitation;
        let bolt_position = if active.kind == StrikeKind::Ground {
            active.top.lerp(active.ground, 0.5)
        } else {
            active.top
        };
        let bolt_intensity = active.intensity
            * direct_share(active.kind)
            * (-f64::from(bolt_position.distance(camera)) / air).exp();
        self.light = WeatherLight {
            position: active.position,
            intensity: active.intensity as f32,
            flash: lightning::flash_at(active.kind, active.brightness, f64::from(distance)) as f32,
            bolt: BoltLight {
                position: bolt_position,
                intensity: bolt_intensity as f32,
            },
            irradiance: (active.intensity * (1000.0 / f64::from(distance)).powi(2)) as f32,
            direction: to_strike / distance,
        };
        if active.kind == StrikeKind::Ground {
            let clarity = (-f64::from(active.ground.distance(camera)) / air).exp() * BOLT_RADIANCE;
            self.bolt_uniforms.origin = (active.ground - camera).to_array();
            self.bolt_uniforms.main = (active.brightness * clarity) as f32;
            self.bolt_uniforms.branches = (active.first_brightness * clarity) as f32;
            // From above the deck the cloud hides the bolt; its glow inside the cloud still shows.
            self.bolt_visible = f64::from(camera.y) < self.clouds.altitude;
        }
    }

    /// Make `strike` the active flash: build its bolt, and cue thunder for a natural one.
    fn show(&mut self, queue: &mut UploadSink<'_>, strike: Strike, thunder: bool) {
        if strike.kind == StrikeKind::Ground {
            let (top, ground) = (strike.top, strike.ground);
            let mut random = Random::new(strike.seed);
            let channel = bolt::build_bolt(
                &mut random,
                [
                    f64::from(top.x - ground.x),
                    f64::from(top.y),
                    f64::from(top.z - ground.z),
                ],
                [0.0, BOLT_FOOT, 0.0],
            );
            self.channel_count = channel.start.len() as u32;
            queue.write_buffer(&self.bolt_start, 0, bytemuck::cast_slice(&channel.start));
            queue.write_buffer(&self.bolt_end, 0, bytemuck::cast_slice(&channel.end));
        }
        if thunder {
            self.thunder
                .push(lightning::thunder_for(strike.kind, strike.distance));
        }
        self.active = Some(strike);
    }

    /// Splashes and the bolt, then the rain over them (the `Transparent` phase).
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, sky_group: &wgpu::BindGroup) {
        if self.splash_count > 0 {
            pass.set_pipeline(&self.splash_pipeline);
            pass.set_bind_group(1, sky_group, &[]);
            pass.set_bind_group(2, &self.splash_group, &[]);
            pass.set_vertex_buffer(0, self.splashes.slice(..));
            pass.draw(0..12, 0..self.splash_count);
        }
        if self.bolt_visible && self.channel_count > 0 {
            pass.set_pipeline(&self.bolt_pipeline);
            pass.set_bind_group(1, sky_group, &[]);
            pass.set_bind_group(2, &self.bolt_group, &[]);
            pass.set_vertex_buffer(0, self.bolt_start.slice(..));
            pass.set_vertex_buffer(1, self.bolt_end.slice(..));
            pass.draw(0..6, 0..self.channel_count);
        }
        if self.rain_count > 0 {
            pass.set_pipeline(&self.rain_pipeline);
            pass.set_bind_group(1, sky_group, &[]);
            pass.set_bind_group(2, &self.rain_group, &[]);
            pass.set_vertex_buffer(0, self.drops.slice(..));
            pass.draw(0..6, 0..self.rain_count);
        }
    }

    /// The rain's streaks into temporal AA's motion pass with `pipeline` (the registry's
    /// [`WeatherPipelines::rain_motion`]): each falls somewhere new every frame, so it takes no history there.
    pub fn draw_motion(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        pipeline: &wgpu::RenderPipeline,
        sky_group: &wgpu::BindGroup,
    ) {
        if self.rain_count > 0 {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(1, sky_group, &[]);
            pass.set_bind_group(2, &self.rain_group, &[]);
            pass.set_vertex_buffer(0, self.drops.slice(..));
            pass.draw(0..6, 0..self.rain_count);
        }
    }

    /// Drops and splashes drawn this frame.
    pub fn counts(&self) -> (u32, u32) {
        (self.rain_count, self.splash_count)
    }
    pub fn draw_layouts(&self) -> &[wgpu::BindGroupLayout; 2] { &self.layouts }

    /// Same visible geometry into the host's temporal rejection target.
    pub fn draw_reactive(&self, pass: &mut wgpu::RenderPass<'_>, sky: &wgpu::BindGroup,
        pipelines: &[wgpu::RenderPipeline; 3]) {
        pass.set_bind_group(1, sky, &[]);
        if self.rain_count > 0 {
            pass.set_pipeline(&pipelines[0]);pass.set_bind_group(2,&self.rain_group,&[]);
            pass.set_vertex_buffer(0,self.drops.slice(..));pass.draw(0..6,0..self.rain_count);
        }
        if self.splash_count > 0 {
            pass.set_pipeline(&pipelines[1]);pass.set_bind_group(2,&self.splash_group,&[]);
            pass.set_vertex_buffer(0,self.splashes.slice(..));pass.draw(0..12,0..self.splash_count);
        }
        if self.bolt_visible && self.channel_count > 0 {
            pass.set_pipeline(&pipelines[2]);pass.set_bind_group(2,&self.bolt_group,&[]);
            pass.set_vertex_buffer(0,self.bolt_start.slice(..));pass.set_vertex_buffer(1,self.bolt_end.slice(..));
            pass.draw(0..6,0..self.channel_count);
        }
    }
}

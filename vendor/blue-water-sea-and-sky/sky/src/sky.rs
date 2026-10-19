//! The sky facade: the scene's state, the atmosphere's tables, the celestial textures, the dome and the
//! environment lighting, on one wgpu device. The atmosphere is takram's (Bruneton's precomputed scattering,
//! `crate::tables`) with the game's grade over it; the rest ports the web game's `sky/Sky.ts` with the parts
//! `celestial/CelestialBodies.ts` and `environment/Environment.ts`.
//!
//! Each frame: [`Sky::update`] writes the uniforms and encodes whatever passes the changes need (the tables
//! when the air changes; the environment when anything it shows does), then [`Sky::draw_dome`] draws the
//! backdrop inside the scene's render pass, and ships and the sea read [`Sky::environment`] and [`Sky::light`].

use glam::{DMat3, DVec3, Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::aerial::AerialCopy;
use crate::environment::{
    CelestialLight, CubeUvLayout, ENVIRONMENT_FORMAT, EnvironmentMap, EnvironmentUniforms,
};
use crate::gpu::{self, workgroups};
use crate::haze::Haze;
use crate::model::{self, AirCoefficients, AtmosphereParameters, sky_grade};
use crate::shaders;
use crate::stars;
use crate::state::{self, CelestialModel, DEFAULT_SKY_SCENE, SkyScene};
use crate::tables::{AtmosphereTables, Precompute, PrecomputePipelines};
use crate::uniforms::{SkyUniforms, v3};
use naval_render::{
    MakePipelines, Pipeline, PipelineId, PipelineRegistry, PlaceholderPipelines, RenderContext,
    ShaderDefValue, ShaderDefs, ShaderError, compute_recipe,
};

const RB: f64 = model::PLANET_RADIUS;
/// The sea-level viewpoint of the environment bake (km).
const SEA_HEIGHT: f64 = 0.002;
/// The twilight lift and the environment follow the camera's altitude in steps of this share of it, or 5 m near
/// the sea.
const REBUILD_SHARE: f64 = 0.02;
const REBUILD_MINIMUM: f64 = 0.005;
/// Highest camera altitude (km) the tables are built for.
const HIGHEST_CAMERA: f64 = 90.0;
/// The sun lights the sky until this far below the horizon; the moon while above `MOON_SETS`.
const SUN_SETS: f64 = -14.0;
const MOON_SETS: f64 = -6.0;
const MOON_FADE: [f64; 2] = [2.0, 8.0];
/// Radiance of the Milky Way's brightest parts on a dark night.
const MILKY_WAY_RADIANCE: f64 = 0.04;
/// Stars are drawn only while the sun is below this elevation (degrees).
const STARS_BELOW: f64 = 1.0;
/// Moon disc radiance per unit of moonlight, and its tight halo (`discs.ts`).
const MOON_DISC: f64 = 2.8;
const MOON_HALO: f64 = 0.06;
const MILKY_WAY_SIZE: (u32, u32) = (2048, 512);
const MILKY_WAY_MIPS: u32 = 12;
/// Blur uniforms sit at this stride in their buffer (the default minimum dynamic-offset alignment).
const BLUR_STRIDE: u64 = 256;
const MAX_BLUR_SAMPLES: usize = 20;

/// Graphics → Clouds, the sky's tier (the web game's `sky/quality.ts`): the environment bake's size and the stars.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkyQuality {
    Low,
    Medium,
    High,
    Ultra,
}

impl SkyQuality {
    /// Width of the web's equirectangular bake, which sets the atlas's face size.
    pub fn environment_width(self) -> u32 {
        match self {
            Self::Low => 256,
            Self::Medium => 384,
            Self::High => 512,
            Self::Ultra => 768,
        }
    }

    pub fn stars(self) -> usize {
        match self {
            Self::Low => 3000,
            Self::Medium => 6000,
            Self::High | Self::Ultra => 9000,
        }
    }
}

/// The formats of the scene pass the dome draws into.
#[derive(Clone, Copy, Debug)]
pub struct SkyTargets {
    pub color: wgpu::TextureFormat,
    /// The scene's reversed-Z depth (`Depth32Float`), or none for a pass without depth.
    pub depth: Option<wgpu::TextureFormat>,
}

impl Default for SkyTargets {
    fn default() -> Self {
        Self {
            color: wgpu::TextureFormat::Rgba16Float,
            depth: Some(wgpu::TextureFormat::Depth32Float),
        }
    }
}

/// The camera the sky draws for.
#[derive(Clone, Copy, Debug)]
pub struct SkyView {
    /// World position (m).
    pub position: Vec3,
    /// Camera → world; the dome uses its rotation.
    pub camera_to_world: Mat4,
    /// Any perspective projection (the game's is reversed-Z); the dome uses its inverse for view rays.
    pub projection: Mat4,
    /// Drawing-buffer pixels.
    pub viewport: [u32; 2],
}

impl SkyView {
    /// A camera at `position` looking at `target` with +Y up, a vertical field of `fov_y_degrees` and an
    /// infinite reversed-Z projection, as the game's camera is.
    pub fn looking_at(
        position: Vec3,
        target: Vec3,
        fov_y_degrees: f32,
        viewport: [u32; 2],
    ) -> Self {
        // Like three's `Matrix4.lookAt` (and `naval_render::Camera::view`), a view straight along the up axis
        // takes +Z or -Z as its up rather than going degenerate.
        let mut forward = target - position;
        if forward.length_squared() < 1e-12 {
            forward = Vec3::NEG_Z;
        }
        let forward = forward.normalize();
        let up = if forward.cross(Vec3::Y).length_squared() < 1e-10 {
            Vec3::new(0.0, 0.0, forward.y.signum())
        } else {
            Vec3::Y
        };
        let world_to_camera = glam::camera::rh::view::look_to_mat4(position, forward, up);
        let aspect = viewport[0] as f32 / viewport[1].max(1) as f32;
        Self {
            position,
            camera_to_world: world_to_camera.inverse(),
            projection: glam::camera::rh::proj::directx::perspective_infinite_reverse(
                fov_y_degrees.to_radians(),
                aspect,
                0.5,
            ),
            viewport,
        }
    }

    fn forward(&self) -> Vec3 {
        self.camera_to_world
            .transform_vector3(Vec3::NEG_Z)
            .normalize()
    }
}

/// Sunlight, moonlight and sky irradiance at the sea, colour × intensity (`AtmospherePart.seaLevel`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SeaLevel {
    pub sun: DVec3,
    pub moon: DVec3,
    pub sky: DVec3,
}

#[derive(Clone, Copy, Debug, Default)]
struct Dirty {
    /// The air changed: the tables are computed again.
    air: bool,
    /// The camera's altitude stepped: the twilight lift and the environment follow it.
    camera: bool,
    environment: bool,
}

struct MilkyWayPasses {
    bake_layout: wgpu::BindGroupLayout,
    down_layout: wgpu::BindGroupLayout,
    bake: wgpu::ComputePipeline,
    downsample: wgpu::ComputePipeline,
    /// The bake's group, then one per mip below it.
    groups: Vec<wgpu::BindGroup>,
    baked: bool,
}

/// One half blur of the prefilter.
struct BlurPass {
    offset: u32,
    /// Reads the atlas and writes the ping-pong target (the latitudinal half), or the other way.
    from_atlas: bool,
    out_size: u32,
}

struct EnvironmentPasses {
    bake_layout: wgpu::BindGroupLayout,
    read_layout: wgpu::BindGroupLayout,
    write_layout: wgpu::BindGroupLayout,
    bake: wgpu::ComputePipeline,
    bake_group: wgpu::BindGroup,
    bake_buffer: wgpu::Buffer,
    blur: wgpu::ComputePipeline,
    read_atlas: wgpu::BindGroup,
    read_ping: wgpu::BindGroup,
    write_atlas: wgpu::BindGroup,
    write_ping: wgpu::BindGroup,
    blurs: Vec<BlurPass>,
    _ping: wgpu::Texture,
    _blur_buffer: wgpu::Buffer,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BakeParams {
    face_size: u32,
    first_row: u32,
    last_row: u32,
    texel_angle: f32,
}

/// What the clouds hand the sky so its dome and environment show them (`naval-clouds` builds it): the light shafts
/// the dome reads at each pixel, and the clouds' short march over the environment bake.
pub struct CloudHooks {
    /// Composable WGSL modules (file path, source) in dependency order. One must be `naval_clouds::hooks` with
    /// `clouds_shadow_length(pixel: vec2<f32>) -> f32` (km of the view ray through that screen pixel lying in the
    /// clouds' shadow, 0 without shafts; the dome passes it to takram's sky radiance, as takram's sky reads its
    /// shadow-length buffer) and `clouds_bake(origin: vec3<f32>, direction: vec3<f32>) -> vec4<f32>` (origin in
    /// world metres at the sea under the camera; premultiplied radiance in game units with aerial perspective in rgb,
    /// transmittance in a). They may import `naval_sky::common` and `naval_sky::atmosphere`.
    pub modules: Vec<(String, String)>,
    /// Their bindings, at group 2 of the dome and the bake.
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
}

/// The sweep that keeps the environment current while clouds drift (`Environment.update`): a band of the atlas
/// every fourth frame, the prefilter after the last of four; a full bake at once after a change or a 2 km jump.
struct Sweep {
    frame: u32,
    baked_origin: DVec3,
}

/// `Environment.ts`: `BANDS` bands over `SWEEP_FRAMES` frames, and the origin jump that re-bakes at once (m).
const BANDS: u32 = 4;
const SWEEP_FRAMES: u32 = 16;
const JUMP: f64 = 2000.0;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Blur {
    weights: [[f32; 4]; 5],
    pole_axis: [f32; 3],
    d_theta: f32,
    texel_size: [f32; 2],
    max_mip: f32,
    mip_int: f32,
    samples: u32,
    latitudinal: u32,
    out_size: u32,
    padding: u32,
    out_origin: [u32; 2],
    padding_tail: [u32; 2],
}

/// The game's sky on one device. See the module docs.
pub struct Sky {
    quality: SkyQuality,
    scene: SkyScene,
    model: CelestialModel,
    air: AirCoefficients,
    sun_elevation: f64,
    sun_lift: f64,
    moon_share: f64,
    sun_active: bool,
    moon_active: bool,
    camera_height: f64,
    sea_level: SeaLevel,
    light: state::Light,
    time: f64,
    uniforms: SkyUniforms,
    dirty: Dirty,

    device: wgpu::Device,
    uniform_buffer: wgpu::Buffer,
    /// The air the tables hold (`naval_sky::common::air`).
    air_buffer: wgpu::Buffer,
    parameters: AtmosphereParameters,
    sky_layout: wgpu::BindGroupLayout,
    sky_group: wgpu::BindGroup,
    star_grid: wgpu::Texture,
    catalog: Vec<stars::Star>,
    tables: AtmosphereTables,
    precompute: Precompute,
    milky_way: MilkyWayPasses,
    _milky_way_texture: wgpu::Texture,
    /// The aerial-perspective volume as the dome and the clouds read it (group 1, binding 12).
    aerial: AerialCopy,
    /// The weather's haze, which the aerial perspective draws and the sun's and moon's light at the sea pass through.
    haze: Haze,
    environment: EnvironmentMap,
    environment_passes: EnvironmentPasses,
    dome: wgpu::RenderPipeline,
    targets: SkyTargets,
    timestamps: Option<wgpu::QuerySet>,
    /// Cloud drift velocity (m/s) in world XZ, from the scene's cloud wind (`SkyState.wind`).
    wind: DVec3,
    /// Lightning this frame, as the weather set it.
    flash: f64,
    bolt: crate::environment::BoltLight,
    clouds: Option<CloudHooks>,
    sweep: Sweep,
    /// Atlas rows the last update baked (0 when it baked none).
    environment_rows: u32,
    /// Times the tables were computed ([`Sky::precomputes`]).
    precomputes: u32,
    /// Built by [`Sky::new_deferred`]: the pipelines are these stand-ins until the first
    /// [`Sky::adopt_pipelines`] takes the registry's.
    placeholders: Option<PlaceholderPipelines>,
}

/// The sky's pipelines on the renderer core's registry, so they rebuild when their WGSL changes on disk (hot reload):
/// made by [`Sky::register_pipelines`] from the library's copies of the sky's files ([`shaders::register`] first),
/// and taken into the sky each frame by [`Sky::adopt_pipelines`] (a rebuild replaces what an id names).
#[derive(Clone, Copy, Debug)]
pub struct SkyPipelines {
    precompute: PrecomputePipelines,
    milky_way_bake: PipelineId,
    milky_way_mip: PipelineId,
    environment_bake: PipelineId,
    environment_blur: PipelineId,
    dome: PipelineId,
}

impl Sky {
    /// Builds the sky's pipelines through the renderer's registry, as [`Sky::new`] and [`Sky::attach_clouds`] built
    /// them (the dome and the bake over the attached clouds' modules, which must be in the library too), for
    /// [`Sky::adopt_pipelines`]. The sky's files must be registered first ([`shaders::register`]). A quality change
    /// keeps the layouts, so the ids stay good; attaching clouds afterwards needs registering again.
    pub fn register_pipelines(
        &self,
        ctx: &mut RenderContext<'_>,
    ) -> Result<SkyPipelines, ShaderError> {
        // The passes leave group 0 (the renderer's camera) empty.
        let none = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("sky no group"),
                entries: &[],
            });
        let clouds_defs: ShaderDefs = if self.clouds.is_some() {
            vec![("CLOUDS".to_owned(), ShaderDefValue::Bool(true))]
        } else {
            Vec::new()
        };
        let precompute = self.precompute.register(ctx)?;
        let mut compute = |label: &str,
                           shader: &str,
                           entry: &'static str,
                           layouts: Vec<wgpu::BindGroupLayout>,
                           defs: ShaderDefs| {
            ctx.add_pipeline(label, shader, defs, compute_recipe(layouts, entry))
        };
        let milky_way_bake = compute(
            "milky way bake",
            shaders::MILKY_WAY_PATH,
            "bake_main",
            vec![self.milky_way.bake_layout.clone()],
            Vec::new(),
        )?;
        let milky_way_mip = compute(
            "milky way mip",
            shaders::MILKY_WAY_PATH,
            "downsample_main",
            vec![self.milky_way.down_layout.clone()],
            Vec::new(),
        )?;
        let passes = &self.environment_passes;
        let environment_bake = compute(
            "sky environment bake",
            shaders::ENVIRONMENT_BAKE_PATH,
            "bake_main",
            vec![
                none.clone(),
                self.sky_layout.clone(),
                self.clouds
                    .as_ref()
                    .map_or_else(|| none.clone(), |hooks| hooks.bind_group_layout.clone()),
                passes.bake_layout.clone(),
            ],
            clouds_defs.clone(),
        )?;
        let environment_blur = compute(
            "sky environment blur",
            shaders::PREFILTER_PATH,
            "blur_main",
            vec![passes.read_layout.clone(), passes.write_layout.clone()],
            Vec::new(),
        )?;
        // As `dome_pipeline` builds it: group 0 left empty (the renderer's camera group, with its shadow maps, would
        // take the stage past its sampled-texture limit beside the sky's and the clouds' groups).
        let sky_layout = self.sky_layout.clone();
        let hooks_layout = self
            .clouds
            .as_ref()
            .map(|hooks| hooks.bind_group_layout.clone());
        let targets = self.targets;
        let dome = ctx.add_pipeline(
            "sky dome",
            shaders::DOME_PATH,
            clouds_defs,
            Box::new(move |build| {
                Pipeline::Render(dome_from_module(
                    build.device,
                    build.module,
                    &sky_layout,
                    targets,
                    hooks_layout.as_ref(),
                    build.cache,
                ))
            }),
        )?;
        Ok(SkyPipelines {
            precompute,
            milky_way_bake,
            milky_way_mip,
            environment_bake,
            environment_blur,
            dome,
        })
    }

    /// Takes the registry's current pipelines (rebuilt after a shader edit) in place of the sky's own, and re-runs
    /// what a changed one baked: the precompute's redo the tables, the milky way its bake, and each of them and the
    /// bake or prefilter the environment. Returns whether any pipeline changed.
    pub fn adopt_pipelines(&mut self, registry: &PipelineRegistry, ids: &SkyPipelines) -> bool {
        fn take(slot: &mut wgpu::ComputePipeline, new: &wgpu::ComputePipeline) -> bool {
            let changed = slot != new;
            if changed {
                *slot = new.clone();
            }
            changed
        }
        let compute = |id| registry.compute(id);
        let air = self.precompute.adopt(registry, &ids.precompute);
        let milky_way = take(&mut self.milky_way.bake, compute(ids.milky_way_bake))
            | take(&mut self.milky_way.downsample, compute(ids.milky_way_mip));
        let environment = take(
            &mut self.environment_passes.bake,
            compute(ids.environment_bake),
        ) | take(
            &mut self.environment_passes.blur,
            compute(ids.environment_blur),
        );
        let dome = registry.render(ids.dome);
        let dome_changed = self.dome != *dome;
        if dome_changed {
            self.dome = dome.clone();
        }
        // A deferred sky's first adoption takes the registry's pipelines in place of its placeholders, not a rebuild:
        // it has computed no tables yet (they wait for this), and tables it took from another sky
        // (`take_tables_from`) were computed with the registry's own.
        if self.placeholders.is_none() {
            self.dirty.air |= air;
        }
        if milky_way {
            self.milky_way.baked = false;
        }
        self.dirty.environment |= air || milky_way || environment;
        // Every pipeline is the registry's now.
        self.placeholders = None;
        air || milky_way || environment || dome_changed
    }

    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        quality: SkyQuality,
        targets: SkyTargets,
    ) -> Self {
        Self::build(device, queue, quality, targets, None)
    }

    /// A sky whose pipelines are `placeholders` until [`Sky::adopt_pipelines`] takes the renderer
    /// registry's ([`Sky::register_pipelines`] builds them on its build threads): its constructor
    /// compiles nothing. Register and adopt before the first update; [`Sky::attach_clouds`] may come
    /// first.
    pub fn new_deferred(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        quality: SkyQuality,
        targets: SkyTargets,
        placeholders: &PlaceholderPipelines,
    ) -> Self {
        Self::build(device, queue, quality, targets, Some(placeholders))
    }

    fn build(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        quality: SkyQuality,
        targets: SkyTargets,
        placeholders: Option<&PlaceholderPipelines>,
    ) -> Self {
        let make = MakePipelines::from_placeholders(placeholders);
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky uniforms"),
            size: std::mem::size_of::<SkyUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // takram's tables (filled by the first update) and the air they hold.
        let tables = AtmosphereTables::new(device);
        let air_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sky air"),
            size: std::mem::size_of::<AtmosphereParameters>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let half = wgpu::TextureFormat::Rgba16Float;

        // The celestial bodies' textures.
        let star_grid = gpu::texture_2d(
            device,
            "star grid",
            stars::STAR_TEXTURE_WIDTH,
            stars::STAR_TEXTURE_HEIGHT,
            1,
            half,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        let catalog = stars::star_catalog();
        write_half_texture(
            queue,
            &star_grid,
            stars::STAR_TEXTURE_WIDTH,
            stars::STAR_TEXTURE_HEIGHT,
            &stars::star_texels(&catalog, quality.stars()),
        );
        let star_ramp = gpu::texture_2d(
            device,
            "star colours",
            stars::RAMP_SIZE,
            1,
            1,
            half,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        write_half_texture(queue, &star_ramp, stars::RAMP_SIZE, 1, &stars::star_ramp());
        let milky_way_texture = gpu::texture_2d(
            device,
            "milky way",
            MILKY_WAY_SIZE.0,
            MILKY_WAY_SIZE.1,
            MILKY_WAY_MIPS,
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        );

        let linear_clamp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("sky linear clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let milky_way_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("milky way"),
            address_mode_u: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        // Group 1 (group 0 is the renderer core's camera): everything the dome, the bake and later parts read.
        let sky_layout = gpu::layout(
            device,
            "sky",
            &[
                gpu::uniform_entry(0, false),
                gpu::texture_entry(1, false),
                gpu::texture_3d_entry(2),
                gpu::texture_3d_entry(3),
                gpu::texture_3d_entry(4),
                gpu::texture_entry(5, true),
                gpu::sampler_entry(6),
                gpu::texture_entry(7, true),
                gpu::texture_entry(8, true),
                gpu::texture_entry(9, true),
                gpu::sampler_entry(10),
                gpu::uniform_entry(11, false),
                AerialCopy::layout_entry(12),
            ],
        );
        let star_grid_view = star_grid.create_view(&Default::default());
        let star_ramp_view = star_ramp.create_view(&Default::default());
        let milky_way_view = milky_way_texture.create_view(&Default::default());
        let aerial = AerialCopy::new(device);
        use wgpu::BindingResource as R;
        let sky_group = gpu::bind_group(
            device,
            "sky",
            &sky_layout,
            &[
                (0, uniform_buffer.as_entire_binding()),
                (1, R::TextureView(&tables.transmittance_view)),
                (2, R::TextureView(&tables.scattering_view)),
                (3, R::TextureView(&tables.single_mie_view)),
                (4, R::TextureView(&tables.higher_order_view)),
                (5, R::TextureView(&tables.irradiance_view)),
                (6, R::Sampler(&linear_clamp)),
                (7, R::TextureView(&star_grid_view)),
                (8, R::TextureView(&star_ramp_view)),
                (9, R::TextureView(&milky_way_view)),
                (10, R::Sampler(&milky_way_sampler)),
                (11, air_buffer.as_entire_binding()),
                (12, aerial.buffer.as_entire_binding()),
            ],
        );

        let precompute = Precompute::new(device, make);
        let milky_way = MilkyWayPasses::new(device, &milky_way_texture, make);
        let layout = CubeUvLayout::from_bake_width(quality.environment_width());
        let environment = EnvironmentMap::new(device, layout);
        let environment_passes =
            EnvironmentPasses::new(device, &sky_layout, &environment, None, make);
        let dome = make.render(|| dome_pipeline(device, &sky_layout, targets, None));

        let air = model::air_coefficients(&DEFAULT_SKY_SCENE.atmosphere);
        let mut sky = Self {
            quality,
            scene: DEFAULT_SKY_SCENE,
            model: CelestialModel::default(),
            air,
            sun_elevation: 90.0,
            sun_lift: 1.0,
            moon_share: 1.0,
            sun_active: true,
            moon_active: true,
            camera_height: 0.03,
            sea_level: SeaLevel {
                sun: DVec3::ZERO,
                moon: DVec3::ZERO,
                sky: DVec3::ZERO,
            },
            light: state::Light {
                direction: DVec3::Y,
                color: DVec3::ONE,
                intensity: 0.0,
                night: false,
            },
            time: 0.0,
            uniforms: SkyUniforms::default(),
            dirty: Dirty {
                air: true,
                camera: true,
                environment: true,
            },
            device: device.clone(),
            uniform_buffer,
            air_buffer,
            parameters: AtmosphereParameters::new(&air),
            sky_layout,
            sky_group,
            star_grid,
            catalog,
            tables,
            precompute,
            milky_way,
            _milky_way_texture: milky_way_texture,
            aerial,
            haze: Haze::default(),
            environment,
            environment_passes,
            dome,
            targets,
            timestamps: None,
            wind: DVec3::ZERO,
            flash: 0.0,
            bolt: Default::default(),
            clouds: None,
            environment_rows: 0,
            precomputes: 0,
            sweep: Sweep {
                frame: 0,
                baked_origin: DVec3::splat(f64::INFINITY),
            },
            placeholders: placeholders.cloned(),
        };
        // No deck: the fog is the clear sky's own.
        sky.uniforms.haze = [0.0, 1.0, 1.0, 0.0];
        sky.set_camera_height(0.03);
        sky.apply(&DEFAULT_SKY_SCENE);
        sky
    }

    pub fn quality(&self) -> SkyQuality {
        self.quality
    }

    pub fn scene(&self) -> &SkyScene {
        &self.scene
    }

    /// The scene's place on the Earth and the sidereal time there, or none (the port, a placeless battle): the stars
    /// and the Milky Way stand by it from the next frame. It outlasts [`Sky::apply`], and costs no bake: set it every
    /// frame and the sky turns with the clock.
    pub fn set_place(&mut self, place: Option<state::Place>) {
        self.model.set_place(place);
    }

    /// Apply a scene description (`Sky.apply`): the bodies, the air, the sea-level light.
    pub fn apply(&mut self, scene: &SkyScene) {
        self.scene = *scene;
        // `SkyState.apply`.
        self.model
            .set(scene.sun_elevation, scene.sun_azimuth, scene.moon_phase);
        if let Some(at) = scene.moon {
            self.model
                .stand_moon(state::direction_from_angles(at.elevation, at.azimuth));
        }
        let u = &mut self.uniforms;
        u.sun_direction = v3(self.model.sun);
        u.moon_direction = v3(self.model.moon);
        u.moon_phase = self.model.phase as f32;
        u.sun_irradiance = [scene.sun_intensity as f32; 3];
        u.moon_irradiance =
            v3(state::moon_tint()
                * (state::MOONLIGHT * state::moon_illumination(self.model.phase)));
        let heading = scene.clouds.wind_heading.to_radians();
        self.wind = DVec3::new(heading.sin(), 0.0, heading.cos()) * scene.clouds.wind_speed;
        // `Atmosphere.apply`.
        let air = model::air_coefficients(&scene.atmosphere);
        if air != self.air {
            self.air = air;
            self.dirty.air = true;
        }
        self.write_air();
        let moon_elevation = self.model.moon.y.clamp(-1.0, 1.0).asin().to_degrees();
        self.sun_elevation = scene.sun_elevation;
        self.sun_lift = self.lift();
        self.moon_share = if moon_elevation > MOON_SETS {
            1.0 - model::smoothstep(MOON_FADE[0], MOON_FADE[1], scene.sun_elevation)
        } else {
            0.0
        };
        self.sun_active = scene.sun_elevation > SUN_SETS;
        self.moon_active = self.moon_share > 0.0;
        self.dirty.environment = true;
        self.light_the_sea();
    }

    /// The sun's, the moon's and the sky's light at the sea for the applied scene, and the scene's light from them.
    fn light_the_sea(&mut self) {
        let scene = self.scene;
        let moon_elevation = self.model.moon.y.clamp(-1.0, 1.0).asin().to_degrees();
        let moon_irradiance = DVec3::from(self.uniforms.moon_irradiance.map(f64::from));
        let (sun, moon) = model::sea_level_light(
            scene.sun_elevation,
            scene.sun_intensity,
            &self.air,
            moon_irradiance.to_array(),
            moon_elevation,
        );
        let sky = model::sky_irradiance(
            &self.air,
            self.model.sun,
            scene.sun_intensity,
            self.sun_lift,
        );
        // The dome and the sky's light are graded `sky_grade::GAIN` over the tables; the sun and moon on the world take
        // the same gain, so the sea, ships, land and clouds sit on the dome's scale (one brightness scale for all). Both
        // reach the sea through the weather's haze, as they light the haze itself (`crate::haze`).
        let gain = sky_grade::GAIN;
        let through = |elevation: f64| {
            self.haze
                .beam_transmittance(&self.air, 0.0, elevation.to_radians().sin())
        };
        self.sea_level = SeaLevel {
            sun: DVec3::from(sun) * gain * through(scene.sun_elevation),
            moon: DVec3::from(moon) * gain * through(moon_elevation),
            sky: DVec3::from(sky),
        };
        self.light = state::choose_light(
            self.model.sun,
            self.model.moon,
            self.sea_level.sun,
            self.sea_level.moon,
        );
        self.refresh();
    }

    /// Advance sky time by `dt` seconds (0 on a paused frame), write the uniforms for `view`, and encode the
    /// passes the changes since the last update need.
    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &SkyView,
        dt: f64,
    ) {
        self.update_tables(queue, encoder, view, dt);
        self.update_environment(queue, encoder);
    }

    /// [`Sky::update_tables`], its pass timed by the renderer core's timer as `sky tables` (in place of
    /// [`Sky::set_timestamps`]' query set), the frame's uniforms written through `uploads` (the renderer core's
    /// staging belt in a layer's `prepare`; the tables' rare precompute writes through `queue`).
    pub fn update_tables_timed(
        &mut self,
        queue: &wgpu::Queue,
        uploads: &mut naval_render::UploadSink<'_>,
        encoder: &mut wgpu::CommandEncoder,
        view: &SkyView,
        dt: f64,
        timer: Option<&naval_render::GpuTimer>,
    ) {
        self.prepare_tables(uploads, view, dt);
        self.encode_tables(queue, encoder, timer);
    }

    /// [`Sky::update_environment`], its pass timed by the renderer core's timer as `sky environment`, its writes
    /// through `uploads`.
    pub fn update_environment_timed(
        &mut self,
        uploads: &mut naval_render::UploadSink<'_>,
        encoder: &mut wgpu::CommandEncoder,
        timer: Option<&naval_render::GpuTimer>,
    ) {
        self.encode_environment(uploads, encoder, timer);
    }

    /// The first half of [`Sky::update`]: advance time and the cloud drift, write the uniforms and encode the
    /// atmosphere's tables. The clouds fill their light tables between it and [`Sky::update_environment`], whose
    /// bake reads them.
    pub fn update_tables(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &SkyView,
        dt: f64,
    ) {
        self.prepare_tables(&mut naval_render::UploadSink::Queue(queue), view, dt);
        self.encode_tables(queue, encoder, None);
    }

    /// Time, the cloud drift and the uniforms for `view`: everything of [`Sky::update_tables`] but its pass.
    fn prepare_tables(
        &mut self,
        uploads: &mut naval_render::UploadSink<'_>,
        view: &SkyView,
        dt: f64,
    ) {
        if dt > 0.0 {
            self.time += dt;
            let drift = DVec3::from(self.uniforms.wind_offset.map(f64::from)) + self.wind * dt;
            self.uniforms.wind_offset = v3(drift);
        }
        // The twilight lift and the sky seen from the camera follow its altitude in steps.
        let altitude = (f64::from(view.position.y) / 1000.0).clamp(SEA_HEIGHT, HIGHEST_CAMERA);
        if (altitude - self.camera_height).abs() > REBUILD_MINIMUM.max(REBUILD_SHARE * altitude) {
            self.set_camera_height(altitude);
            self.sun_lift = self.lift();
            self.dirty.camera = true;
            self.dirty.environment = true;
        }
        self.refresh();
        self.write_view(view);
        self.write_celestial(view);
        uploads.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&self.uniforms));
    }

    /// The second half of [`Sky::update`]: the environment, re-baked and refiltered when anything it shows
    /// changed, and with clouds attached swept band by band as they drift.
    pub fn update_environment(&mut self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder) {
        self.encode_environment(&mut naval_render::UploadSink::Queue(queue), encoder, None);
    }

    /// Draws the dome into the scene's render pass: at the far plane, only where nothing else drew.
    pub fn draw_dome(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.dome);
        pass.set_bind_group(1, &self.sky_group, &[]);
        if let Some(clouds) = &self.clouds {
            pass.set_bind_group(2, &clouds.bind_group, &[]);
        }
        pass.draw(0..3, 0..1);
    }

    /// Shows the clouds in the dome (the high sheet) and the environment (the sheet and the clouds' march), and
    /// starts the environment's sweep. Rebuilds the dome's and the bake's pipelines over `hooks.modules`.
    pub fn attach_clouds(&mut self, device: &wgpu::Device, hooks: CloudHooks) {
        let placeholders = self.placeholders.clone();
        let make = MakePipelines::from_placeholders(placeholders.as_ref());
        self.dome =
            make.render(|| dome_pipeline(device, &self.sky_layout, self.targets, Some(&hooks)));
        self.environment_passes = EnvironmentPasses::new(
            device,
            &self.sky_layout,
            &self.environment,
            Some(&hooks),
            make,
        );
        self.clouds = Some(hooks);
        self.dirty.environment = true;
    }

    /// A new bind group for the attached clouds (after they replace a texture), same layout.
    pub fn set_cloud_bind_group(&mut self, bind_group: wgpu::BindGroup) {
        if let Some(clouds) = &mut self.clouds {
            clouds.bind_group = bind_group;
        }
    }

    pub fn has_clouds(&self) -> bool {
        self.clouds.is_some()
    }

    /// Lightning this frame (the weather's strike): the channel lighting the clouds and its radiant intensity,
    /// the scene flash (a multiple of the ambient) and the flash's direct light, for [`Sky::lightning`].
    pub fn set_lightning(
        &mut self,
        position: Vec3,
        intensity: f32,
        flash: f32,
        bolt: crate::environment::BoltLight,
    ) {
        self.uniforms.lightning_position = position.to_array();
        self.uniforms.lightning_intensity = intensity;
        self.uniforms.flash = flash;
        self.flash = f64::from(flash);
        self.bolt = bolt;
    }

    /// How the air under a closed deck greys (0–1) and dims the clear sky's haze toward the horizon
    /// (`LayerUniforms.haze`): the fog sampler's and the clouds' `overcast`.
    pub fn set_haze(&mut self, grey: f32, shade: f32) {
        self.uniforms.haze[0] = grey;
        self.uniforms.haze[1] = shade;
    }

    /// The dome's night glow and night floor as a share of the authored level (1 is the authored sky, 0 a black
    /// moonless night). The environment bake keeps the full glow, so the world's night light does not change; the
    /// stars and the Milky Way are not dimmed. Rides in `SkyUniforms.haze.z`.
    pub fn set_night_sky_brightness(&mut self, scale: f32) {
        self.uniforms.haze[2] = if scale.is_finite() { scale.clamp(0.0, 1.0) } else { 1.0 };
    }

    /// The accumulated cloud drift (m, world XZ).
    pub fn wind_offset(&self) -> Vec3 {
        Vec3::from(self.uniforms.wind_offset)
    }

    /// Sky time (s): star twinkle, cloud boiling.
    pub fn time(&self) -> f64 {
        self.time
    }

    /// The prefiltered environment ships and the sea read (see `native/docs/sky.md`).
    pub fn environment(&self) -> &EnvironmentMap {
        &self.environment
    }

    /// The active celestial light (`CelestialLight`): the scene's directional light.
    pub fn light(&self) -> CelestialLight {
        CelestialLight {
            direction: self.light.direction.as_vec3(),
            color: self.light.color.as_vec3(),
            intensity: self.light.intensity as f32,
            night: self.light.night,
        }
    }

    /// The sun's disc as the dome draws it (`sun_disc` in `celestial.wgsl`: the sun's irradiance times its `SUN_DISC`,
    /// limb darkened, through the air to space and the weather's haze, seen from the sea): its angular radius
    /// (radians) and its radiance averaged over it (linear RGB), before clouds or anything else in front; none below
    /// the horizon. Photo mode's lens sets what the frame shows of it against this (`naval_render::photo_optics`).
    pub fn drawn_sun(&self) -> (f64, DVec3) {
        // `DISC_RADIUS`, `SUN_DISC` and `SUN_LIMB` in celestial.wgsl.
        const DISC_RADIUS: f64 = 0.012_247_448_713_915_89;
        const SUN_DISC: f64 = 1.35;
        const SUN_LIMB: [f64; 3] = [0.5, 0.6, 0.72];
        if !self.sun_active || self.sun_elevation <= 0.0 {
            return (DISC_RADIUS, DVec3::ZERO);
        }
        let air = model::sea_transmittance(&self.air, self.sun_elevation);
        let mu = self.sun_elevation.to_radians().sin();
        let haze = (-self.haze.extinction_at_sea(&self.air) * self.haze.path(0.0, mu)).exp();
        // The limb's darkening averaged over the disc (the cosine across a disc averages 2/3).
        let radiance = DVec3::from_array(std::array::from_fn(|c| {
            self.scene.sun_intensity * SUN_DISC * (1.0 - SUN_LIMB[c] / 3.0) * air[c] * haze
        }));
        (DISC_RADIUS, radiance)
    }

    /// Lightning's light on the scene this frame (`CelestialLight.flash` and `.bolt` on the web), as the weather
    /// last set it; zero without weather.
    pub fn lightning(&self) -> crate::environment::LightningLight {
        crate::environment::LightningLight {
            flash: self.flash as f32,
            bolt: self.bolt,
        }
    }

    pub fn sea_level(&self) -> SeaLevel {
        self.sea_level
    }

    /// The air the tables were computed for (per km), for CPU reads of its transmittance (`model::optical_depth`).
    pub fn air(&self) -> &AirCoefficients {
        &self.air
    }

    /// Unit vectors toward the sun and the moon.
    pub fn sun_direction(&self) -> Vec3 {
        self.model.sun.as_vec3()
    }

    pub fn moon_direction(&self) -> Vec3 {
        self.model.moon.as_vec3()
    }

    /// Toward the celestial pole the sun and the stars turn about: the place's, or the authored sun's.
    pub fn celestial_pole(&self) -> Vec3 {
        self.model.north().as_vec3()
    }

    /// The aerial-perspective volume's copy the dome and the clouds read (group 1, binding 12): the pass that fills
    /// the renderer's volume copies it here ([`AerialCopy::copy_from`]).
    pub fn aerial(&self) -> &AerialCopy {
        &self.aerial
    }

    /// The weather's haze (`crate::haze`).
    pub fn weather_haze(&self) -> Haze {
        self.haze
    }

    /// Sets the weather's haze: the aerial perspective draws it, and the sun's and moon's light at the sea is taken
    /// again through it. The environment holds no haze, so it is not baked again (a weather's drift sets the haze every
    /// frame). Returns whether the haze changed.
    pub fn set_weather_haze(&mut self, haze: Haze) -> bool {
        if haze == self.haze {
            return false;
        }
        self.haze = haze;
        self.light_the_sea();
        true
    }

    /// The sky's group-1 layout and bind group (`shaders/common.wgsl`): the uniforms, the atmosphere's tables
    /// and the celestial textures, for a pass of another part that imports `naval_sky::atmosphere`.
    pub fn bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.sky_layout
    }

    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.sky_group
    }

    /// The uniforms as last written, for diagnostics.
    pub fn uniforms(&self) -> &SkyUniforms {
        &self.uniforms
    }

    /// Profiling: a timestamp query set of at least four entries the update's passes write, 0 and 1 at the
    /// start and end of the tables' pass, 2 and 3 around the environment's; a skipped pass writes none. The
    /// device needs `Features::TIMESTAMP_QUERY`.
    pub fn set_timestamps(&mut self, query_set: Option<wgpu::QuerySet>) {
        self.timestamps = query_set;
    }

    /// Stands sky time (star twinkle, cloud boiling) at `seconds` and the clouds drifted there from the start at
    /// the current wind, for a host whose clock is absolute (`SkyState.seek`); a later `update` adds its `dt`.
    pub fn seek(&mut self, seconds: f64) {
        self.time = seconds;
        self.uniforms.wind_offset = v3(self.wind * seconds);
    }

    /// Stands the cloud drift at `offset` (m, world XZ), as a capture read back from the web places it.
    pub fn set_wind_offset(&mut self, offset: Vec3) {
        self.uniforms.wind_offset = [offset.x, 0.0, offset.z];
    }

    /// Atlas rows the last update baked into the environment: all of them after a change, a quarter on a sweep's
    /// band frame, 0 when it baked none (then its timestamps, if set, were not written).
    pub fn environment_rows(&self) -> u32 {
        self.environment_rows
    }

    /// Diagnostics: how many times this sky has computed its atmosphere tables (once, unless its air changed or it took
    /// another sky's: [`Sky::take_tables_from`]).
    pub fn precomputes(&self) -> u32 {
        self.precomputes
    }

    /// Re-bakes and refilters the environment on the next update.
    pub fn refresh_environment(&mut self) {
        self.dirty.environment = true;
    }

    /// A sky put on screen in place of `from` (the theatre's scene swaps): when this one has not computed its tables yet
    /// and `from`'s hold the same air, copies them on `encoder` (about 25 MB) instead of the precompute its first update
    /// would run (75 to 110 ms of GPU, the swap's longest frame). Submit `encoder` before this sky's next update. Whether
    /// it copied them.
    pub fn take_tables_from(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        from: &Sky,
    ) -> bool {
        if !self.dirty.air || from.dirty.air || self.parameters != from.parameters {
            return false;
        }
        self.tables.copy_from(encoder, &from.tables);
        queue.write_buffer(&self.air_buffer, 0, bytemuck::bytes_of(&self.parameters));
        self.dirty.air = false;
        self.dirty.environment = true;
        true
    }

    /// Graphics → Clouds: the environment's size and the stars drawn. The environment's texture changes
    /// with the size: rebuild bind groups that hold its view.
    pub fn set_quality(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, quality: SkyQuality) {
        if quality == self.quality {
            return;
        }
        assert!(
            self.placeholders.is_none(),
            "Sky::set_quality before the registry's pipelines were adopted (Sky::new_deferred): \
             adopt them first"
        );
        self.quality = quality;
        write_half_texture(
            queue,
            &self.star_grid,
            stars::STAR_TEXTURE_WIDTH,
            stars::STAR_TEXTURE_HEIGHT,
            &stars::star_texels(&self.catalog, quality.stars()),
        );
        let layout = CubeUvLayout::from_bake_width(quality.environment_width());
        if layout != self.environment.layout {
            let generation = self.environment.generation;
            self.environment = EnvironmentMap::new(device, layout);
            self.environment.generation = generation;
            self.environment_passes = EnvironmentPasses::new(
                device,
                &self.sky_layout,
                &self.environment,
                self.clouds.as_ref(),
                MakePipelines::Now,
            );
        }
        self.dirty.environment = true;
    }

    /// The authored terms the dome applies over the tables, and the air the tables are computed for.
    fn write_air(&mut self) {
        let air = &self.air;
        let u = &mut self.uniforms;
        let f = |c: [f64; 3]| c.map(|v| v as f32);
        u.multiple = air.multiple as f32;
        u.saturation = (sky_grade::SATURATION * air.chroma) as f32;
        let (lobe, core) = model::phase_terms(air.mie_g, air.mie_gain);
        u.phase_lobe = f(lobe);
        u.phase_core = f(core);
        self.parameters = AtmosphereParameters::new(air);
    }

    fn set_camera_height(&mut self, altitude: f64) {
        self.camera_height = altitude;
        let u = &mut self.uniforms;
        u.camera_height = altitude as f32;
        u.camera_horizon_cosine = horizon_cosine_of(altitude) as f32;
    }

    /// The twilight lift for the sun's elevation above the camera's geometric horizon.
    fn lift(&self) -> f64 {
        model::twilight_lift(
            self.sun_elevation + (-horizon_cosine_of(self.camera_height)).asin().to_degrees(),
        )
    }

    /// Each light's terms: the tables read in the game's units, with the dome's grade for the sky it draws.
    fn refresh(&mut self) {
        let u = &mut self.uniforms;
        let scale = model::game_scale(self.scene.sun_intensity);
        let balance = model::white_balance();
        let sky: [f64; 3] =
            std::array::from_fn(|c| model::sky_relative_luminance()[c] * balance[c]);
        let sun: [f64; 3] =
            std::array::from_fn(|c| model::sun_relative_luminance()[c] * balance[c]);
        let solar = model::solar_luminance();
        let sun_on = if self.sun_active { 1.0 } else { 0.0 };
        let dome = scale * self.sun_lift * sky_grade::GAIN * sun_on;
        u.sun_scale = sky.map(|c| (c * dome) as f32);
        // The moon as a sun of its own colour (white balance and scale cancel): per channel, its irradiance over the
        // sun's in the tables' units.
        let moon = u.moon_irradiance.map(f64::from);
        let share = sky_grade::MOON * sky_grade::GAIN * self.moon_share;
        let sky_factors = model::sky_relative_luminance();
        u.moon_scale =
            std::array::from_fn(|c| (sky_factors[c] * moon[c] / solar[c] * share) as f32);
        // The clouds' sunlight on the dome's scale too (as the world's: `SeaLevel`).
        u.direct_scale = sun.map(|c| (c * scale * self.sun_lift * sky_grade::GAIN) as f32);
        u.diffuse_scale = sky.map(|c| (c * scale * self.sun_lift * sky_grade::GAIN) as f32);
        u.game_scale = scale as f32;
        u.sun_lift = self.sun_lift as f32;
        u.sun_elevation = (self.sun_elevation
            + (-horizon_cosine_of(self.camera_height)).asin().to_degrees())
            as f32;
        u.sun_active = sun_on as f32;
        u.moon_active = if self.moon_active { 1.0 } else { 0.0 };
        u.light_direction = v3(self.light.direction);
        u.light_color = v3(self.light.color * self.light.intensity);
        u.time = self.time as f32;
    }

    fn write_view(&mut self, view: &SkyView) {
        let u = &mut self.uniforms;
        u.camera_position = view.position.to_array();
        u.inverse_projection = view.projection.inverse().to_cols_array_2d();
        let mut rotation = view.camera_to_world;
        rotation.w_axis = glam::Vec4::W;
        u.view_to_world = rotation.to_cols_array_2d();
        let (w, h) = (view.viewport[0] as f32, view.viewport[1].max(1) as f32);
        u.viewport = [w, h, 1.0 / w.max(1.0), 1.0 / h];
    }

    /// The celestial part's per-frame values (`CelestialBodies.update`).
    fn write_celestial(&mut self, view: &SkyView) {
        let u = &mut self.uniforms;
        let tan_half_field = 1.0 / f64::from(view.projection.y_axis.y);
        let (pixel_angle, star_gain) =
            state::view_scale(tan_half_field, f64::from(view.viewport[1]));
        u.pixel_angle = pixel_angle as f32;
        u.star_gain = star_gain as f32;
        u.forward = view.forward().to_array();
        u.star_horizon = state::horizon_cut(f64::from(view.position.y)) as f32;
        let galactic: DMat3 = state::galactic_rotation(self.model.star_rotation);
        let rows = galactic.transpose();
        u.galactic_x = v3(rows.x_axis);
        u.galactic_y = v3(rows.y_axis);
        u.galactic_z = v3(rows.z_axis);
        let sun_elevation = self.model.sun.y.clamp(-1.0, 1.0).asin().to_degrees();
        let moonlit = state::moonlight(self.model.phase, self.model.moon);
        u.night = if sun_elevation < STARS_BELOW {
            1.0
        } else {
            0.0
        };
        u.star_limit = state::star_limit(sun_elevation, moonlit) as f32;
        u.milky_way = (MILKY_WAY_RADIANCE * state::milky_way_share(sun_elevation, moonlit)) as f32;
        let moon = state::moon_lighting(
            self.model.sun,
            self.model.moon,
            self.model.north(),
            self.model.phase,
            DVec3::from(u.moon_irradiance.map(f64::from)),
            MOON_DISC,
        );
        u.moon_lit = moon.lit as f32;
        u.moon_opposition = moon.opposition as f32;
        u.moon_glow = (MOON_HALO * moon.lit.sqrt()) as f32;
        u.moon_right = v3(moon.right);
        u.moon_up = v3(moon.up);
        u.moon_sun = v3(moon.sun);
        u.moon_surface = v3(moon.surface);
        u.earthshine = v3(moon.earthshine);
        let texel = 2.0 * std::f64::consts::PI / f64::from(self.quality.environment_width());
        u.bake_spread = texel as f32;
        u.bake_level = (texel / (2.0 * std::f64::consts::PI / f64::from(MILKY_WAY_SIZE.0)))
            .log2()
            .max(0.0) as f32;
    }

    /// Timestamp writes for a pass: the renderer core's timer when given, else [`Sky::set_timestamps`]' query set at
    /// `first` and `first + 1`.
    fn pass_timestamps<'a>(
        own: Option<&'a wgpu::QuerySet>,
        timer: Option<&'a naval_render::GpuTimer>,
        label: &str,
        first: u32,
    ) -> Option<wgpu::ComputePassTimestampWrites<'a>> {
        if let Some(timer) = timer {
            return timer.compute_pass(label);
        }
        own.map(|query_set| wgpu::ComputePassTimestampWrites {
            query_set,
            beginning_of_pass_write_index: Some(first),
            end_of_pass_write_index: Some(first + 1),
        })
    }

    /// The Milky Way's bake (once) and, when the air changed, takram's tables computed again (with the pipelines of
    /// a deferred sky only once they are the registry's).
    fn encode_tables(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        timer: Option<&naval_render::GpuTimer>,
    ) {
        if !self.milky_way.baked {
            self.dirty.environment = true;
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sky tables"),
                timestamp_writes: Self::pass_timestamps(
                    self.timestamps.as_ref(),
                    timer,
                    "sky tables",
                    0,
                ),
            });
            self.milky_way.encode(&mut pass);
        }
        if self.dirty.air && self.placeholders.is_none() {
            queue.write_buffer(&self.air_buffer, 0, bytemuck::bytes_of(&self.parameters));
            self.precompute.encode(
                &self.device,
                queue,
                encoder,
                &self.tables,
                &self.parameters,
                Self::pass_timestamps(self.timestamps.as_ref(), timer, "sky precompute", 0),
            );
            self.dirty.air = false;
            self.dirty.environment = true;
            self.precomputes += 1;
        }
        self.dirty.camera = false;
    }

    /// Re-bakes and refilters the whole environment in the next update, then sweeps on from there.
    fn encode_environment(
        &mut self,
        uploads: &mut naval_render::UploadSink<'_>,
        encoder: &mut wgpu::CommandEncoder,
        timer: Option<&naval_render::GpuTimer>,
    ) {
        let face = self.environment.layout.face_size;
        let rows = 2 * face;
        let origin =
            DVec3::from(self.uniforms.camera_position.map(f64::from)) * DVec3::new(1.0, 0.0, 1.0);
        let jumped = self.clouds.is_some() && origin.distance(self.sweep.baked_origin) > JUMP;
        // What to bake this frame: every row and the prefilter after a change, one band while clouds drift.
        let (band, prefilter) = if self.dirty.environment || jumped {
            self.sweep.frame = 0;
            self.sweep.baked_origin = origin;
            (Some((0, rows)), true)
        } else if self.clouds.is_some() {
            let frame = self.sweep.frame;
            self.sweep.frame = (frame + 1) % SWEEP_FRAMES;
            let stride = SWEEP_FRAMES / BANDS;
            if !frame.is_multiple_of(stride) {
                (None, false)
            } else {
                let band = frame / stride;
                let last = band == BANDS - 1;
                if last {
                    self.sweep.baked_origin = origin;
                }
                (Some((rows * band / BANDS, rows * (band + 1) / BANDS)), last)
            }
        } else {
            (None, false)
        };
        self.dirty.environment = false;
        self.environment_rows = band.map_or(0, |(first, last)| last - first);
        if let Some((first_row, last_row)) = band {
            let texel_angle = std::f32::consts::FRAC_PI_2 / (face - 2) as f32;
            uploads.write_buffer(
                &self.environment_passes.bake_buffer,
                0,
                bytemuck::bytes_of(&BakeParams {
                    face_size: face,
                    first_row,
                    last_row,
                    texel_angle,
                }),
            );
            if prefilter {
                self.environment.generation += 1;
            }
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("sky environment"),
                timestamp_writes: Self::pass_timestamps(
                    self.timestamps.as_ref(),
                    timer,
                    "sky environment",
                    2,
                ),
            });
            self.environment_passes.encode_bake(
                &mut pass,
                &self.sky_group,
                self.clouds.as_ref().map(|clouds| &clouds.bind_group),
                face,
                last_row - first_row,
            );
            if prefilter {
                self.environment_passes.encode_prefilter(&mut pass);
            }
        }
        let uniforms = EnvironmentUniforms::new(
            &self.environment.layout,
            self.environment.generation,
            &self.light(),
            self.sun_direction(),
            self.moon_direction(),
            self.model.phase as f32,
        );
        uploads.write_buffer(&self.environment.uniforms, 0, bytemuck::bytes_of(&uniforms));
    }
}

/// Zenith cosine of the geometric horizon from an altitude (km).
fn horizon_cosine_of(altitude: f64) -> f64 {
    -(altitude * (altitude + 2.0 * RB)).sqrt() / (altitude + RB)
}

fn write_half_texture(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
    data: &[u16],
) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(data),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 8),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

impl MilkyWayPasses {
    fn new(device: &wgpu::Device, texture: &wgpu::Texture, make: MakePipelines<'_>) -> Self {
        use wgpu::BindingResource as R;
        let module = make.module(|| {
            shaders::shader_module(
                device,
                shaders::MILKY_WAY_WGSL,
                "naval-sky/shaders/milky_way.wgsl",
            )
        });
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let bake_layout = gpu::layout(device, "milky way bake", &[gpu::storage_entry(0, format)]);
        let down_layout = gpu::layout(
            device,
            "milky way mip",
            &[gpu::storage_entry(0, format), gpu::texture_entry(1, false)],
        );
        let bake = make.compute(|| {
            gpu::compute_pipeline(
                device,
                "milky way bake",
                module.as_ref().expect("built now"),
                "bake_main",
                &[Some(&bake_layout)],
            )
        });
        let downsample = make.compute(|| {
            gpu::compute_pipeline(
                device,
                "milky way mip",
                module.as_ref().expect("built now"),
                "downsample_main",
                &[Some(&down_layout)],
            )
        });
        let level = |mip: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: mip,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let mut groups = vec![gpu::bind_group(
            device,
            "milky way bake",
            &bake_layout,
            &[(0, R::TextureView(&level(0)))],
        )];
        for mip in 1..MILKY_WAY_MIPS {
            groups.push(gpu::bind_group(
                device,
                "milky way mip",
                &down_layout,
                &[
                    (0, R::TextureView(&level(mip))),
                    (1, R::TextureView(&level(mip - 1))),
                ],
            ));
        }
        Self {
            bake_layout,
            down_layout,
            bake,
            downsample,
            groups,
            baked: false,
        }
    }

    fn encode(&mut self, pass: &mut wgpu::ComputePass<'_>) {
        pass.set_pipeline(&self.bake);
        pass.set_bind_group(0, &self.groups[0], &[]);
        pass.dispatch_workgroups(
            workgroups(MILKY_WAY_SIZE.0, 8),
            workgroups(MILKY_WAY_SIZE.1, 8),
            1,
        );
        pass.set_pipeline(&self.downsample);
        for mip in 1..MILKY_WAY_MIPS {
            let (w, h) = (
                (MILKY_WAY_SIZE.0 >> mip).max(1),
                (MILKY_WAY_SIZE.1 >> mip).max(1),
            );
            pass.set_bind_group(0, &self.groups[mip as usize], &[]);
            pass.dispatch_workgroups(workgroups(w, 8), workgroups(h, 8), 1);
        }
        self.baked = true;
    }
}

/// Directions the prefilter's blur poles turn among from level to level (`POLES` in Environment.ts), in this
/// atlas's frame. three stores its atlas y-negated (it looks a render-target atlas up with `vec3(x, -y, z)`),
/// so its blur turns about each pole with y negated as seen in the world; negating y here blurs the same
/// world directions the same way.
fn poles() -> [Vec3; 10] {
    let phi = (1.0 + 5f32.sqrt()) / 2.0;
    let inv = 1.0 / phi;
    [
        Vec3::new(-phi, inv, 0.0),
        Vec3::new(phi, inv, 0.0),
        Vec3::new(-inv, 0.0, phi),
        Vec3::new(inv, 0.0, phi),
        Vec3::new(0.0, phi, -inv),
        Vec3::new(0.0, phi, inv),
        Vec3::new(-1.0, 1.0, -1.0),
        Vec3::new(1.0, 1.0, -1.0),
        Vec3::new(-1.0, 1.0, 1.0),
        Vec3::new(1.0, 1.0, 1.0),
    ]
    .map(|pole| (pole * Vec3::new(1.0, -1.0, 1.0)).normalize())
}

/// The prefilter's half blurs, in order: three's `_blur` per level with the Gaussian chain's incremental
/// sigma, latitudinal (atlas → ping-pong) then longitudinal (ping-pong → atlas).
pub(crate) fn blur_chain(layout: &CubeUvLayout) -> Vec<(Blur, bool)> {
    let levels = layout.levels();
    let poles = poles();
    let n = levels.len();
    let mut chain = Vec::with_capacity(2 * (n - 1));
    for i in 1..n {
        let sigma = (levels[i].sigma.powi(2) - levels[i - 1].sigma.powi(2)).sqrt();
        let pole = poles[(n - i - 1) % poles.len()];
        chain.push((half_blur(layout, i - 1, i, sigma, true, pole), true));
        chain.push((half_blur(layout, i, i, sigma, false, pole), false));
    }
    chain
}

fn half_blur(
    layout: &CubeUvLayout,
    lod_in: usize,
    lod_out: usize,
    sigma: f32,
    latitudinal: bool,
    pole: Vec3,
) -> Blur {
    let levels = layout.levels();
    let pixels = (levels[lod_in].size - 1) as f32;
    let radians_per_pixel = std::f32::consts::PI / (2.0 * pixels);
    let sigma_pixels = sigma / radians_per_pixel;
    let samples = (1 + (3.0 * sigma_pixels).floor() as usize).min(MAX_BLUR_SAMPLES);
    let mut weights = [0f32; MAX_BLUR_SAMPLES];
    let mut sum = 0.0;
    for (i, weight) in weights.iter_mut().enumerate() {
        let x = i as f32 / sigma_pixels;
        *weight = (-x * x / 2.0).exp();
        if i == 0 {
            sum += *weight;
        } else if i < samples {
            sum += 2.0 * *weight;
        }
    }
    for weight in &mut weights {
        *weight /= sum;
    }
    let mut packed = [[0f32; 4]; 5];
    for (i, weight) in weights.iter().enumerate() {
        packed[i / 4][i % 4] = *weight;
    }
    let out = levels[lod_out];
    Blur {
        weights: packed,
        pole_axis: pole.to_array(),
        d_theta: radians_per_pixel,
        texel_size: [1.0 / layout.width as f32, 1.0 / layout.height as f32],
        max_mip: layout.max_mip as f32,
        mip_int: layout.max_mip as f32 - lod_in as f32,
        samples: samples as u32,
        latitudinal: u32::from(latitudinal),
        out_size: out.size,
        padding: 0,
        out_origin: [out.x, out.y],
        padding_tail: [0; 2],
    }
}

impl EnvironmentPasses {
    fn new(
        device: &wgpu::Device,
        sky_layout: &wgpu::BindGroupLayout,
        map: &EnvironmentMap,
        clouds: Option<&CloudHooks>,
        make: MakePipelines<'_>,
    ) -> Self {
        use wgpu::BindingResource as R;
        let layout = map.layout;
        let bake_module = make.module(|| match clouds {
            Some(hooks) => shaders::shader_module_with(
                device,
                shaders::ENVIRONMENT_BAKE_WGSL,
                "naval-sky/shaders/environment_bake.wgsl",
                &hooks.modules,
                &[("CLOUDS", naga_oil::compose::ShaderDefValue::Bool(true))],
            ),
            None => shaders::shader_module(
                device,
                shaders::ENVIRONMENT_BAKE_WGSL,
                "naval-sky/shaders/environment_bake.wgsl",
            ),
        });
        let bake_layout = gpu::layout(
            device,
            "sky environment bake",
            &[
                gpu::storage_entry(0, ENVIRONMENT_FORMAT),
                gpu::uniform_entry(1, false),
            ],
        );
        let bake = make.compute(|| {
            gpu::compute_pipeline(
                device,
                "sky environment bake",
                bake_module.as_ref().expect("built now"),
                "bake_main",
                &[
                    None,
                    Some(sky_layout),
                    clouds.map(|hooks| &hooks.bind_group_layout),
                    Some(&bake_layout),
                ],
            )
        });
        let bake_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sky environment bake"),
            contents: bytemuck::bytes_of(&BakeParams {
                face_size: layout.face_size,
                first_row: 0,
                last_row: 2 * layout.face_size,
                texel_angle: std::f32::consts::FRAC_PI_2 / (layout.face_size - 2) as f32,
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let atlas_storage = map.texture.create_view(&Default::default());
        let bake_group = gpu::bind_group(
            device,
            "sky environment bake",
            &bake_layout,
            &[
                (0, R::TextureView(&atlas_storage)),
                (1, bake_buffer.as_entire_binding()),
            ],
        );

        let ping = gpu::texture_2d(
            device,
            "sky environment ping-pong",
            layout.width,
            layout.height,
            1,
            ENVIRONMENT_FORMAT,
            wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let ping_view = ping.create_view(&Default::default());
        let chain = blur_chain(&layout);
        let mut blur_bytes = vec![0u8; BLUR_STRIDE as usize * chain.len()];
        for (index, (blur, _)) in chain.iter().enumerate() {
            let start = index * BLUR_STRIDE as usize;
            blur_bytes[start..start + std::mem::size_of::<Blur>()]
                .copy_from_slice(bytemuck::bytes_of(blur));
        }
        let blur_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("sky environment blurs"),
            contents: &blur_bytes,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let blur_module = make.module(|| {
            shaders::shader_module(
                device,
                shaders::PREFILTER_WGSL,
                "naval-sky/shaders/prefilter.wgsl",
            )
        });
        let read_layout = gpu::layout(
            device,
            "sky environment blur read",
            &[
                gpu::texture_entry(0, true),
                gpu::sampler_entry(1),
                gpu::uniform_entry(2, true),
            ],
        );
        let write_layout = gpu::layout(
            device,
            "sky environment blur write",
            &[gpu::storage_entry(0, ENVIRONMENT_FORMAT)],
        );
        let blur = make.compute(|| {
            gpu::compute_pipeline(
                device,
                "sky environment blur",
                blur_module.as_ref().expect("built now"),
                "blur_main",
                &[Some(&read_layout), Some(&write_layout)],
            )
        });
        let blur_binding = wgpu::BufferBinding {
            buffer: &blur_buffer,
            offset: 0,
            size: wgpu::BufferSize::new(std::mem::size_of::<Blur>() as u64),
        };
        let read = |view: &wgpu::TextureView, label: &str| {
            gpu::bind_group(
                device,
                label,
                &read_layout,
                &[
                    (0, R::TextureView(view)),
                    (1, R::Sampler(&map.sampler)),
                    (2, R::Buffer(blur_binding.clone())),
                ],
            )
        };
        let write = |view: &wgpu::TextureView, label: &str| {
            gpu::bind_group(device, label, &write_layout, &[(0, R::TextureView(view))])
        };
        let blurs = chain
            .iter()
            .enumerate()
            .map(|(index, (blur, from_atlas))| BlurPass {
                offset: (index as u64 * BLUR_STRIDE) as u32,
                from_atlas: *from_atlas,
                out_size: blur.out_size,
            })
            .collect();
        Self {
            bake_layout,
            read_layout: read_layout.clone(),
            write_layout: write_layout.clone(),
            bake,
            bake_group,
            bake_buffer,
            blur,
            read_atlas: read(&map.view, "sky environment blur read atlas"),
            read_ping: read(&ping_view, "sky environment blur read ping-pong"),
            write_atlas: write(&atlas_storage, "sky environment blur write atlas"),
            write_ping: write(&ping_view, "sky environment blur write ping-pong"),
            blurs,
            _ping: ping,
            _blur_buffer: blur_buffer,
        }
    }

    /// Level 0's rows from the bake buffer's `first_row`, `rows` of them.
    fn encode_bake(
        &self,
        pass: &mut wgpu::ComputePass<'_>,
        sky_group: &wgpu::BindGroup,
        clouds: Option<&wgpu::BindGroup>,
        face_size: u32,
        rows: u32,
    ) {
        pass.set_pipeline(&self.bake);
        pass.set_bind_group(1, sky_group, &[]);
        if let Some(clouds) = clouds {
            pass.set_bind_group(2, clouds, &[]);
        }
        pass.set_bind_group(3, &self.bake_group, &[]);
        pass.dispatch_workgroups(workgroups(3 * face_size, 8), workgroups(rows, 8), 1);
    }

    /// The Gaussian chain over every level below level 0.
    fn encode_prefilter(&self, pass: &mut wgpu::ComputePass<'_>) {
        pass.set_pipeline(&self.blur);
        for blur in &self.blurs {
            let (read, write) = if blur.from_atlas {
                (&self.read_atlas, &self.write_ping)
            } else {
                (&self.read_ping, &self.write_atlas)
            };
            pass.set_bind_group(0, read, &[blur.offset]);
            pass.set_bind_group(1, write, &[]);
            pass.dispatch_workgroups(
                workgroups(3 * blur.out_size, 8),
                workgroups(2 * blur.out_size, 8),
                1,
            );
        }
    }
}

fn dome_pipeline(
    device: &wgpu::Device,
    sky_layout: &wgpu::BindGroupLayout,
    targets: SkyTargets,
    clouds: Option<&CloudHooks>,
) -> wgpu::RenderPipeline {
    let path = "naval-sky/shaders/dome.wgsl";
    let module = match clouds {
        Some(hooks) => shaders::shader_module_with(
            device,
            shaders::DOME_WGSL,
            path,
            &hooks.modules,
            &[("CLOUDS", naga_oil::compose::ShaderDefValue::Bool(true))],
        ),
        None => shaders::shader_module(device, shaders::DOME_WGSL, path),
    };
    dome_from_module(
        device,
        &module,
        sky_layout,
        targets,
        clouds.map(|hooks| &hooks.bind_group_layout),
        None,
    )
}

/// The dome's pipeline from its composed module: group 0 empty, the sky at 1, the clouds' hooks at 2.
fn dome_from_module(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    sky_layout: &wgpu::BindGroupLayout,
    targets: SkyTargets,
    hooks_layout: Option<&wgpu::BindGroupLayout>,
    cache: Option<&wgpu::PipelineCache>,
) -> wgpu::RenderPipeline {
    let groups = match hooks_layout {
        Some(hooks) => vec![None, Some(sky_layout), Some(hooks)],
        None => vec![None, Some(sky_layout)],
    };
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sky dome"),
        bind_group_layouts: &groups,
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sky dome"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("dome_vertex"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        // Reversed Z: the dome sits at depth 0 (far) and passes only where the scene left the clear value.
        depth_stencil: targets.depth.map(|format| wgpu::DepthStencilState {
            format,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("dome_fragment"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: targets.color,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_camera_looking_straight_up_or_down_stays_finite() {
        let eye = Vec3::new(0.0, 30.0, 0.0);
        for target in [Vec3::ZERO, Vec3::new(0.0, 60.0, 0.0), eye] {
            let view = SkyView::looking_at(eye, target, 52.0, [960, 540]);
            assert!(view.camera_to_world.is_finite(), "{target:?}");
            assert!(view.forward().is_finite());
        }
        let down = SkyView::looking_at(eye, Vec3::ZERO, 52.0, [960, 540]);
        assert!((down.forward() - Vec3::NEG_Y).length() < 1e-5);
    }

    #[test]
    fn blur_chain_matches_three_on_high() {
        let layout = CubeUvLayout::from_bake_width(512);
        let chain = blur_chain(&layout);
        assert_eq!(chain.len(), 18);
        // Level 1 from level 0: sigma 1/64 over 128-texel faces, 1.26 pixels, four samples.
        let (first, from_atlas) = chain[0];
        assert!(from_atlas);
        assert_eq!(first.samples, 4);
        assert_eq!(first.mip_int, 7.0);
        assert_eq!(first.out_size, 64);
        assert_eq!(first.out_origin, [0, 256]);
        // The roughest level: sigma √(0.582² − 0.526²) at 16², eight samples.
        let (last, _) = chain[17];
        assert_eq!(last.samples, 8);
        assert_eq!(last.mip_int, -2.0);
        assert_eq!(last.out_origin, [288, 448]);
        let weights: f32 = (0..20)
            .map(|i| last.weights[i / 4][i % 4] * if i == 0 { 1.0 } else { 2.0 })
            .take(8)
            .sum();
        assert!((weights - 1.0).abs() < 1e-5);
    }
}

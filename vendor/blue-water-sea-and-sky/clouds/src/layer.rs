//! The volumetric clouds on the GPU, a port of @takram/three-clouds (`CloudsEffect`, `CloudsPass`, `ShadowPass`): the
//! Beer shadow maps over cascades with their temporal pass, the march over a quarter-resolution buffer one Bayer cell
//! per update with its temporal upscaling, light shafts and haze, and the composite over the scene. The game adds the
//! ground shadow map ships and the sea read. A frame is [`CloudLayer::prepare`] (the plan and the uniforms),
//! [`CloudLayer::encode_shadows`] before the scene pass, [`CloudLayer::encode_march`] once the opaque scene's depth
//! is copied, then [`CloudLayer::draw_composite`] in the scene pass after the dome.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use naval_render::{
    MakePipelines, Pipeline, PipelineId, PipelineRecipe, PipelineRegistry, PlaceholderPipelines,
    RenderContext, ShaderError, compute_recipe,
};
use naval_sky::{CloudHooks, Sky, SkyQuality, SkyScene};

use crate::Targets;
use crate::cascades::{Cascade, CascadeSettings, Cascades, MAX_CASCADES, light_distance};
use crate::detail_noise::{self, DetailFill, DetailPipelines};
use crate::gpu::{self, Texture, workgroups};
use crate::layers::Layers;
use crate::quality::{self, CloudTier};
use crate::{shaders, smoothstep, textures};

/// `CloudUniforms` in `shaders/common.wgsl`, field for field.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct CloudUniforms {
    pub view: [[f32; 4]; 4],
    pub previous_view_projection: [[f32; 4]; 4],
    pub camera: [f32; 4],
    pub forward: [f32; 4],
    pub screen: [f32; 4],
    /// Full output viewport in xy; screen.xy is the independently scaled cloud buffer.
    pub viewport: [f32; 4],
    pub frame: [f32; 4],
    pub planes: [f32; 4],
    pub sun: [f32; 4],
    pub drift: [f32; 4],
    pub medium: [f32; 4],
    pub repeats: [f32; 4],
    pub min_layer_heights: [f32; 4],
    pub max_layer_heights: [f32; 4],
    pub min_interval_heights: [f32; 4],
    pub max_interval_heights: [f32; 4],
    pub density_scales: [f32; 4],
    pub shape_amounts: [f32; 4],
    pub shape_detail_amounts: [f32; 4],
    pub weather_exponents: [f32; 4],
    pub shape_altering_biases: [f32; 4],
    pub coverage_filter_widths: [f32; 4],
    pub heights: [f32; 4],
    pub shadow_layer_mask: [f32; 4],
    pub density_exp_terms: [f32; 4],
    pub density_exponents: [f32; 4],
    pub density_linear_terms: [f32; 4],
    pub density_constant_terms: [f32; 4],
    pub channels: [[f32; 4]; 4],
    pub scattering: [f32; 4],
    pub phase: [f32; 4],
    pub march: [f32; 4],
    pub march_limits: [f32; 4],
    pub secondary: [f32; 4],
    pub shafts: [f32; 4],
    pub haze: [f32; 4],
    pub switches: [f32; 4],
    pub state: [f32; 4],
    pub shadow_map: [f32; 4],
    pub shadow_march: [f32; 4],
    pub shadow_limits: [f32; 4],
    pub shadow_intervals: [[f32; 4]; MAX_CASCADES],
    pub shadow_matrices: [[[f32; 4]; 4]; MAX_CASCADES],
    pub inverse_shadow_matrices: [[[f32; 4]; 4]; MAX_CASCADES],
    pub shadow_reprojection: [[[f32; 4]; 4]; MAX_CASCADES],
    pub ground: [f32; 4],
    pub bake: [f32; 4],
    /// x the cascades the shadow pass marches this update (a bit each: all after a cut, else the round robin's and any
    /// the view has left), y how many.
    pub cascades: [f32; 4],
    /// xyz toward the body whose light the ground shadow map stops: the sun, or the moon once it outshines the sun (the
    /// light ships, the land and the sea take). w unused.
    pub ground_light: [f32; 4],
    /// Each layer's coverage (takram's one `coverage`, `medium.z`, for all four): the weather's layers
    /// (`crate::weathers`).
    pub layer_coverages: [f32; 4],
    /// 1 for the layers that are closed slabs (`crate::layers::Layer::slab`).
    pub layer_slabs: [f32; 4],
    /// xyz how far the wind carried the clouds since the previous update (m, world; 0 without history), which the march's
    /// motion takes so the reconstruction follows a drifting cloud. w unused.
    pub drift_step: [f32; 4],
    /// The clouds' key light ([`crate::key_light::KeyLight::uniform`]; `sun` points at it): xyz the moon's direct scale,
    /// w 1 while the moon is the key light.
    pub key_light: [f32; 4],
    /// x 1 while a lightning flash's light on the clouds changes (this update's or the last's lit them): the
    /// reconstruction clips its history as a moving view's. yzw unused.
    pub flash: [f32; 4],
    /// Live experiments: debug view, bypass Beer attenuation, far-ray step multiplier, mean local sun depth.
    pub experiments: [f32; 4],
}

/// Optional host experiments. Defaults retain the native tier's rendering and budgets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloudTuning {
    pub debug: u8,
    pub shadow_mode: u8,
    /// Independent cascade fitting distance (m); zero uses the scene camera.
    pub shadow_far: f32,
    pub far_step_scale: f32,
    /// -1 uses the tier; 0 keeps Beer shadows but skips the local sunlight march.
    pub sun_steps: i32,
}

impl Default for CloudTuning {
    fn default() -> Self {
        Self { debug: 0, shadow_mode: 0, shadow_far: 0.0, far_step_scale: 1.0, sun_steps: -1 }
    }
}

impl CloudTuning {
    fn sanitize(mut self) -> Self {
        self.debug = self.debug.min(5);
        self.shadow_mode = self.shadow_mode.min(1);
        self.shadow_far = if self.shadow_far.is_finite() { self.shadow_far.clamp(0.0, 200_000.0) } else { 0.0 };
        self.far_step_scale = if self.far_step_scale.is_finite() { self.far_step_scale.clamp(1.0, 4.0) } else { 1.0 };
        self.sun_steps = self.sun_steps.clamp(-1, 8);
        self
    }

    /// Only fitting uses this projection. Scene depth, rays and reprojection retain the host camera.
    fn shadow_view(self, view: &CloudView) -> CloudView {
        if self.shadow_far <= 0.0 { return *view; }
        let far = self.shadow_far.max(view.near * 2.0);
        let projection = glam::camera::rh::proj::directx::perspective(
            2.0 * view.tan_half_fov().atan(),
            view.projection.y_axis.y / view.projection.x_axis.x,
            far,
            view.near,
        );
        CloudView { projection, far, ..*view }
    }
}

/// The camera the clouds march for this frame.
#[derive(Clone, Copy, Debug)]
pub struct CloudView {
    /// World position (m).
    pub position: Vec3,
    /// World → view.
    pub view: Mat4,
    /// View → clip, reversed depth, without temporal anti-aliasing's jitter.
    pub projection: Mat4,
    pub near: f32,
    pub far: f32,
    /// Drawing-buffer pixels.
    pub viewport: [u32; 2],
}

impl CloudView {
    /// From the renderer core's camera block. Its projection carries temporal AA's jitter when that is on; the clouds
    /// march their own sub-pixel pattern, so a perspective camera's projection is rebuilt without it.
    pub fn from_camera(camera: &naval_render::CameraUniforms) -> Self {
        let mut projection = Mat4::from_cols_array(&camera.projection);
        if camera.tan_half_fov_y > 0.0 && camera.aspect > 0.0 {
            projection = naval_render::camera::reversed_perspective(
                2.0 * camera.tan_half_fov_y.atan(),
                camera.aspect,
                camera.near,
                camera.far,
            );
        }
        Self {
            position: Vec3::from(camera.position),
            view: Mat4::from_cols_array(&camera.view),
            projection,
            near: camera.near,
            far: camera.far,
            viewport: [camera.viewport[0] as u32, camera.viewport[1] as u32],
        }
    }

    pub fn view_projection(&self) -> Mat4 {
        self.projection * self.view
    }

    /// Tangent of half the vertical field of view.
    pub fn tan_half_fov(&self) -> f32 {
        1.0 / self.projection.y_axis.y
    }
}

/// `naval_clouds::shadow`'s uniforms (`CloudShadow` in `shaders/shadow.wgsl`), 64 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct CloudShadowUniforms {
    /// Toward the body whose light the map stops: the sun, or the moon once it outshines the sun.
    pub sun_direction: [f32; 3],
    pub strength: f32,
    pub wind_offset: [f32; 2],
    pub camera: [f32; 2],
    pub size: f32,
    pub enabled: f32,
    pub padding: [f32; 2],
    /// The port's berth clearing: centre x, z (m), reach and fade (m); a fade of 0 clears nothing
    /// ([`CloudLayer::set_clearing`]).
    pub clearing: [f32; 4],
}

/// The cloud shadow ships, islands and the sea read (see `native/docs/clouds.md`): bind `bind_group` (laid out by
/// `bind_group_layout`: 0 the map, 1 its linear repeat sampler, 2 [`CloudShadowUniforms`]) in any group and call
/// `naval_clouds::shadow::cloud_shadow` (register it with [`crate::shaders::add_shadow_module`]).
pub struct CloudShadow {
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    pub uniforms: wgpu::Buffer,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
}

/// A clearing in the clouds' shadow (`PORT_CLEARING` in the web game's `VisualEnvironment.ts`): no cloud shades within
/// `reach` metres of `center` (world XZ), the shadow back in full by `fade`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CloudClearing {
    pub center: glam::Vec2,
    pub reach: f32,
    pub fade: f32,
}

impl CloudClearing {
    /// The port's: within 350 m of the berth, back by 900 m (`PORT_CLEARING`).
    pub fn port(berth: glam::Vec2) -> Self {
        Self {
            center: berth,
            reach: 350.0,
            fade: 900.0,
        }
    }
}

/// The planet's radius (m): the sea, as the atmosphere's bottom.
pub const PLANET_RADIUS: f32 = 6_360_000.0;
/// One tile of the local weather (m): takram's 100 repeats per face of a cube-sphere earth.
pub const WEATHER_TILE: f32 = 100_000.0;
/// takram's `shapeRepeat` and `shapeDetailRepeat` (per m) and `turbulenceRepeat` (per weather tile).
const SHAPE_REPEAT: f32 = 0.0003;
const SHAPE_DETAIL_REPEAT: f32 = 0.01;
const TURBULENCE_REPEAT: f32 = 20.0;
/// takram's `turbulenceDisplacement` (m), `coverage` default, scattering and absorption coefficients.
const TURBULENCE_DISPLACEMENT: f32 = 350.0;
const SCATTERING_COEFFICIENT: f32 = 1.0;
const ABSORPTION_COEFFICIENT: f32 = 0.0;
/// takram's scattering: sky light scale, ground bounce scale, powder scale and exponent (slabs only since 2026-10-04:
/// `shaders/march.wgsl`, `sample_radiance`), and the phase's two anisotropies and their mix.
const SKY_LIGHT_SCALE: f32 = 1.0;
const GROUND_BOUNCE_SCALE: f32 = 1.0;
const POWDER_SCALE: f32 = 0.8;
const POWDER_EXPONENT: f32 = 150.0;
const SCATTER_ANISOTROPY: [f32; 3] = [0.7, -0.2, 0.5];
/// takram's haze: density scale, exponent, scattering and absorption coefficients.
const HAZE: [f32; 4] = [3e-5, 1e-3, 0.9, 0.5];
/// takram's `maxShadowFilterRadius` (texels), `SHADOW_SAMPLE_COUNT` and `opticalDepthTailScale`.
const MAX_SHADOW_FILTER_RADIUS: f32 = 6.0;
const SHADOW_SAMPLE_COUNT: f32 = 8.0;
const OPTICAL_DEPTH_TAIL_SCALE: f32 = 2.0;
/// takram's `mipLevelScale` with temporal upscaling ("not exactly").
const MIP_LEVEL_SCALE: f32 = 0.25;
/// The 4 × 4 Bayer order the march's cell follows over 16 updates (takram's `bayerIndices`).
const BAYER_INDICES: [u32; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];
/// Updates a still or held view marches before it stands: 15 full Bayer cycles, so every pixel's mean holds 16 samples
/// with the first update's full march (`resolve.wgsl`'s `MOST_SAMPLES`; 48 updates, three cycles, before 2026-10-03).
pub const HOLD_UPDATES: u32 = 240;
/// Ground shadow texels marched per update once the map is whole.
const GROUND_TEXELS_PER_UPDATE: u32 = 16384;
/// The shadow fades in with the sun from 1° to 7° up (the sine of its elevation).
const SHADOW_SUN: [f64; 2] = [0.02, 0.12];
/// Radians the light may turn since the ground map was last marched before it is marched whole: far beyond a frame's
/// motion in play (the sun's 15° an hour is about 1e-6 a frame, 1e-3 even at a thousandfold clock), so only an edit to
/// the sky reaches it; a smaller edit leaves the shadows of a paused picture at most half a degree off.
const GROUND_TURN: f32 = 1e-2;

/// The update's cell of the 4 × 4 block, x right and y down.
fn bayer_cell(frame: u32) -> [u32; 2] {
    let index = BAYER_INDICES
        .iter()
        .position(|&value| value == frame % 16)
        .expect("every index") as u32;
    [index % 4, index / 4]
}

/// Two view-projections equal to float noise.
fn same_view(a: &Mat4, b: &Mat4) -> bool {
    a.to_cols_array()
        .iter()
        .zip(b.to_cols_array().iter())
        .all(|(x, y)| (x - y).abs() <= 1e-6 * (1.0 + x.abs()))
}

/// What this frame's update does, decided in `prepare`.
#[derive(Clone, Copy, Debug, Default)]
struct Plan {
    /// March, reconstruct and update the shadow maps.
    update: bool,
    /// The shadow maps' history the temporal pass reads; it writes the other, which the march then reads.
    shadow_from: usize,
    /// How many cascades the shadow pass marches this update (which: `clouds.cascades.x`, a bit each); the temporal
    /// pass carries the others' history over unchanged.
    shadow_count: u32,
    /// The clouds' history the reconstruction reads; it writes the other.
    clouds_from: usize,
    /// The ground shadow texels this update marches: first and count.
    ground: Option<(u32, u32)>,
    /// The first update after a cut: march every pixel into the history the composite reads, no reconstruction.
    full: bool,
    march_size: [u32; 2],
    screen_size: [u32; 2],
}

/// The layer's pipelines on the renderer core's registry (hot reload): made by [`CloudLayer::register_pipelines`],
/// taken in each frame by [`CloudLayer::adopt_pipelines`].
#[derive(Clone, Debug)]
pub struct CloudPipelines {
    shadow: PipelineId,
    shadow_resolve: PipelineId,
    ground: PipelineId,
    march: PipelineId,
    march_full: PipelineId,
    resolve: PipelineId,
    composite: PipelineId,
    turbulence: PipelineId,
    detail: DetailPipelines,
}

/// The turbulence's fill (`noise.wgsl`): its group, its pipeline and whether the texture holds its output.
struct TurbulenceFill {
    layout: wgpu::BindGroupLayout,
    group: wgpu::BindGroup,
    pipeline: wgpu::ComputePipeline,
    filled: bool,
}

impl TurbulenceFill {
    fn new(device: &wgpu::Device, texture: &Texture, make: MakePipelines<'_>) -> Self {
        let layout = gpu::layout(
            device,
            "cloud turbulence fill",
            &[gpu::storage_2d_entry(3, wgpu::TextureFormat::Rgba8Unorm)],
        );
        let group = gpu::bind_group(
            device,
            "cloud turbulence fill",
            &layout,
            &[(3, wgpu::BindingResource::TextureView(&texture.view))],
        );
        let pipeline = make.compute(|| {
            let module =
                shaders::shader_module(device, shaders::NOISE_WGSL, shaders::NOISE_PATH, &[]);
            gpu::compute_pipeline(
                device,
                "cloud turbulence",
                &module,
                "turbulence_main",
                &[Some(&layout)],
            )
        });
        Self {
            layout,
            group,
            pipeline,
            filled: false,
        }
    }

    fn encode(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("cloud turbulence"),
            timestamp_writes: None,
        });
        pass.set_bind_group(0, &self.group, &[]);
        pass.set_pipeline(&self.pipeline);
        let groups = workgroups(textures::TURBULENCE_SIZE, 8);
        pass.dispatch_workgroups(groups, groups, 1);
        drop(pass);
        self.filled = true;
    }
}

/// The targets that follow the drawing buffer's size: the march's and the reconstruction's.
struct ScreenTargets {
    march_color: Texture,
    march_depth_velocity: Texture,
    march_shadow_length: Texture,
    history_color: [Texture; 2],
    history_shadow_length: [Texture; 2],
}

impl ScreenTargets {
    fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let (low_width, low_height) = (width.div_ceil(4).max(1), height.div_ceil(4).max(1));
        let half = wgpu::TextureFormat::Rgba16Float;
        let target = |label: &str, w: u32, h: u32, format| gpu::target(device, label, w, h, format);
        Self {
            march_color: target("cloud march colour", low_width, low_height, half),
            march_depth_velocity: target(
                "cloud march depth and motion",
                low_width,
                low_height,
                wgpu::TextureFormat::Rgba32Float,
            ),
            march_shadow_length: target("cloud march shaft length", low_width, low_height, half),
            history_color: [
                target("cloud history colour 0", width, height, half),
                target("cloud history colour 1", width, height, half),
            ],
            history_shadow_length: [
                target("cloud history shaft length 0", width, height, half),
                target("cloud history shaft length 1", width, height, half),
            ],
        }
    }
}

/// The Beer shadow maps: this update's march (and its motion) and the two histories the temporal pass alternates.
struct ShadowTargets {
    current: Texture,
    depth_velocity: Texture,
    history: [Texture; 2],
}

impl ShadowTargets {
    fn new(device: &wgpu::Device, size: u32, cascades: usize) -> Self {
        let layers = cascades.max(1) as u32;
        let make = |label: &str| {
            let texture = gpu::texture(
                device,
                label,
                (size, size, layers),
                wgpu::TextureDimension::D2,
                1,
                wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
            );
            let view = texture.texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
            Texture {
                texture: texture.texture,
                view,
            }
        };
        Self {
            current: make("cloud shadow maps (march)"),
            depth_velocity: make("cloud shadow maps (motion)"),
            history: [make("cloud shadow maps 0"), make("cloud shadow maps 1")],
        }
    }
}

/// The march's group 2 for a depth texture: the field, a shadow-map history, the scene's depth.
struct MarchGroups {
    depth_key: u64,
    groups: [wgpu::BindGroup; 2],
}

/// The cloud layer. See the module docs.
pub struct CloudLayer {
    quality: SkyQuality,
    render_scale: f32,
    tuning: CloudTuning,
    tier: CloudTier,
    layers: Layers,
    coverage: f32,
    /// The port's berth clearing in the shadow other layers read ([`CloudLayer::set_clearing`]).
    clearing: Option<CloudClearing>,
    uniforms: CloudUniforms,
    buffer: wgpu::Buffer,
    // Textures.
    weather: Texture,
    shape: Texture,
    detail: Texture,
    turbulence: Texture,
    blue_noise: Texture,
    turbulence_fill: TurbulenceFill,
    detail_fill: DetailFill,
    shadow_targets: ShadowTargets,
    ground_map: Texture,
    screen: ScreenTargets,
    /// A depth texture with nothing in it, for marches without a scene.
    empty_depth: Texture,
    /// The ocean's underwater probe (`set_underwater_probe`), else `no_probe`.
    underwater_probe: wgpu::TextureView,
    /// A 1 × 1 zero: no ray starts under water.
    no_probe: Texture,
    samplers: [wgpu::Sampler; 2],
    // Layouts and groups.
    field_layout: wgpu::BindGroupLayout,
    field_group: wgpu::BindGroup,
    /// The field group changed since [`CloudLayer::take_field_group`] last handed it out.
    field_changed: bool,
    shadow_field_layout: wgpu::BindGroupLayout,
    shadow_field_group: wgpu::BindGroup,
    march_layout: wgpu::BindGroupLayout,
    march_groups: Option<MarchGroups>,
    uniforms_layout: wgpu::BindGroupLayout,
    uniforms_group: wgpu::BindGroup,
    shadow_out_layout: wgpu::BindGroupLayout,
    shadow_out_group: wgpu::BindGroup,
    shadow_resolve_layout: wgpu::BindGroupLayout,
    shadow_resolve_groups: [wgpu::BindGroup; 2],
    ground_out_layout: wgpu::BindGroupLayout,
    ground_out_group: wgpu::BindGroup,
    march_out_layout: wgpu::BindGroupLayout,
    march_out_group: wgpu::BindGroup,
    /// The full march's outputs: the drawing buffer's history `i` (colour, shafts).
    march_full_out_groups: [wgpu::BindGroup; 2],
    resolve_layout: wgpu::BindGroupLayout,
    resolve_groups: [wgpu::BindGroup; 2],
    composite_layout: wgpu::BindGroupLayout,
    composite_groups: [wgpu::BindGroup; 2],
    /// The opaque scene's depth the composite tests the clouds against, and the key of the depth it holds (0 the
    /// layer's own empty depth, else the copy's version + 1).
    composite_depth_layout: wgpu::BindGroupLayout,
    composite_depth: (u64, wgpu::BindGroup),
    shadow_interface: CloudShadow,
    shadow_interface_layout: wgpu::BindGroupLayout,
    // Pipelines.
    shadow_pipeline: wgpu::ComputePipeline,
    shadow_resolve_pipeline: wgpu::ComputePipeline,
    ground_pipeline: wgpu::ComputePipeline,
    march_pipeline: wgpu::ComputePipeline,
    march_full_pipeline: wgpu::ComputePipeline,
    resolve_pipeline: wgpu::ComputePipeline,
    composite_pipeline: wgpu::RenderPipeline,
    /// The scene pass the composite draws into.
    targets: Targets,
    // State (`CloudsEffect.update`).
    width: u32,
    height: u32,
    /// Updates since the sequence started (the Bayer cell, the blue noise's frame).
    updates: u32,
    /// The clouds' history holding the latest reconstruction, and whether it is worth reprojecting.
    current: usize,
    has_history: bool,
    /// What the composite draws, published for the transparents drawn after it ([`CloudLayer::cover`]), and a count
    /// of the screen targets made, so they know to rebind the histories.
    cover: naval_sky::CloudCover,
    screens_made: u64,
    /// The shadow maps' history holding the latest pass, and whether it is.
    shadow_latest: usize,
    shadow_has_history: bool,
    /// The ground map must be marched whole (a new scene, sun or view).
    ground_stale: bool,
    /// The ground map was marched toward the moon (the sky's light is the moon's), not the sun.
    ground_night: bool,
    /// The direction the ground map was last marched toward (a slice or the whole of it).
    ground_direction: Vec3,
    /// Whether the clouds' key light was the moon at the last prepare ([`crate::key_light::KeyLight`]).
    moon_key: bool,
    /// Lightning's intensity on the clouds at the last prepare (the sky's `lightning_intensity`), and where it stood
    /// (`lightning_position`: a strike forced elsewhere in a held picture can come at the same intensity).
    flash_intensity: f32,
    flash_position: [f32; 3],
    ground_next: u32,
    active: bool,
    held: bool,
    held_updates: u32,
    held_view: Mat4,
    /// This frame's time stands still while frames go on (a paused picture: [`CloudLayer::set_still`]).
    still: bool,
    /// This frame leaves time where it was ([`CloudLayer::set_time_stands`]).
    time_stands: bool,
    /// Updates marched since the view last changed while time stood still; at `HOLD_UPDATES` the march stops.
    still_updates: u32,
    still_view: Mat4,
    previous_view_projection: Mat4,
    /// The clouds' drift (m, world) the previous update marched them at.
    previous_drift: Vec3,
    /// Each cascade's world → clip and its inverse as it was last marched: what every pass samples it with, since a
    /// cascade waits up to `cascade_count − 1` updates for its next march (one cascade an update, round robin).
    drawn_shadow_matrices: [Mat4; MAX_CASCADES],
    drawn_inverse_shadow_matrices: [Mat4; MAX_CASCADES],
    drawn_cascades: [Cascade; MAX_CASCADES],
    /// Updates of the shadow maps since the round robin started, which picks the cascade to march.
    shadow_rounds: u32,
    plan: Plan,
    timestamps: Option<wgpu::QuerySet>,
}

impl CloudLayer {
    /// A layer whose composite draws into `targets` (the renderer core's `HDR_FORMAT` and reversed depth), over the
    /// sky's group-1 bindings (`sky.bind_group_layout()`). Uploads the textures and fills the turbulence at once.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sky: &Sky,
        quality: SkyQuality,
        targets: Targets,
    ) -> Self {
        Self::build(device, queue, sky, quality, targets, MakePipelines::Now)
    }

    /// The same with `placeholders` for its pipelines until [`CloudLayer::adopt_pipelines`] takes the renderer
    /// registry's ([`CloudLayer::register_pipelines`] builds them on its build threads). The turbulence is filled at
    /// the first [`CloudLayer::encode_noise_fill`].
    pub fn new_deferred(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sky: &Sky,
        quality: SkyQuality,
        targets: Targets,
        placeholders: &PlaceholderPipelines,
    ) -> Self {
        Self::build(
            device,
            queue,
            sky,
            quality,
            targets,
            MakePipelines::Deferred(placeholders),
        )
    }

    fn build(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sky: &Sky,
        quality: SkyQuality,
        targets: Targets,
        make: MakePipelines<'_>,
    ) -> Self {
        use wgpu::BindingResource as R;
        use wgpu::TextureViewDimension as D;
        let tier = quality::tier(quality);

        let weather = textures::local_weather(device, queue);
        let shape = textures::shape(device, queue);
        let detail = detail_noise::texture(device);
        let turbulence = textures::turbulence(device);
        let blue_noise = textures::blue_noise(device, queue);
        let mut turbulence_fill = TurbulenceFill::new(device, &turbulence, make);
        let mut detail_fill = DetailFill::new(device, &detail, make);
        if make.is_now() {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("cloud noise fills"),
            });
            turbulence_fill.encode(&mut encoder);
            detail_fill.encode_if_stale(&mut encoder);
            queue.submit([encoder.finish()]);
        }
        let shadow_targets =
            ShadowTargets::new(device, tier.shadow.map_size, tier.shadow.cascade_count);
        let ground_map = gpu::target(
            device,
            "cloud ground shadow",
            tier.ground_size,
            tier.ground_size,
            wgpu::TextureFormat::Rgba8Unorm,
        );
        let screen = ScreenTargets::new(device, 1, 1);
        let empty_depth = gpu::texture(
            device,
            "cloud empty depth",
            (1, 1, 1),
            wgpu::TextureDimension::D2,
            1,
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let no_probe = gpu::texture(
            device,
            "cloud no underwater probe",
            (1, 1, 1),
            wgpu::TextureDimension::D2,
            1,
            wgpu::TextureFormat::Rgba16Float,
            wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let underwater_probe = no_probe.view.clone();

        let sampler = |label: &str, mode: wgpu::AddressMode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: mode,
                address_mode_v: mode,
                address_mode_w: mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            })
        };
        let samplers = [
            sampler("cloud repeat", wgpu::AddressMode::Repeat),
            sampler("cloud clamp", wgpu::AddressMode::ClampToEdge),
        ];
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cloud uniforms"),
            size: std::mem::size_of::<CloudUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Group 2 variants: what each pass reads of `common.wgsl`'s bindings.
        let field_entries = || {
            vec![
                gpu::uniform_entry(0),
                gpu::texture_2d_entry(1),
                gpu::texture_entry(2, D::D3, true),
                gpu::texture_entry(3, D::D3, true),
                gpu::texture_2d_entry(4),
                gpu::texture_2d_entry(5),
                gpu::sampler_entry(7),
                gpu::sampler_entry(8),
            ]
        };
        let shadow_field_layout = gpu::layout(device, "cloud field", &field_entries());
        let mut hooks_entries = field_entries();
        hooks_entries.extend([gpu::texture_2d_entry(9), gpu::texture_2d_entry(10)]);
        let field_layout = gpu::layout(device, "cloud field and shafts", &hooks_entries);
        let mut march_entries = field_entries();
        march_entries.extend([
            gpu::texture_entry(6, D::D2Array, true),
            gpu::depth_entry(11),
            gpu::texture_2d_entry(12),
        ]);
        let march_layout =
            gpu::layout(device, "cloud field, shadow maps and depth", &march_entries);
        let uniforms_layout = gpu::layout(
            device,
            "cloud uniforms",
            &[
                gpu::uniform_entry(0),
                gpu::sampler_entry(7),
                gpu::sampler_entry(8),
            ],
        );
        let half = wgpu::TextureFormat::Rgba16Float;
        let shadow_out_layout = gpu::layout(
            device,
            "cloud shadow maps out",
            &[
                gpu::storage_entry(0, half, D::D2Array),
                gpu::storage_entry(1, half, D::D2Array),
            ],
        );
        let shadow_resolve_layout = gpu::layout(
            device,
            "cloud shadow maps resolve",
            &[
                gpu::texture_entry(0, D::D2Array, false),
                gpu::texture_entry(1, D::D2Array, false),
                gpu::texture_entry(2, D::D2Array, true),
                gpu::storage_entry(3, half, D::D2Array),
            ],
        );
        let ground_out_layout = gpu::layout(
            device,
            "cloud ground shadow out",
            &[gpu::storage_2d_entry(0, wgpu::TextureFormat::Rgba8Unorm)],
        );
        let march_out_layout = gpu::layout(
            device,
            "cloud march out",
            &[
                gpu::storage_2d_entry(0, half),
                gpu::storage_2d_entry(1, wgpu::TextureFormat::Rgba32Float),
                gpu::storage_2d_entry(2, half),
            ],
        );
        let resolve_layout = gpu::layout(
            device,
            "cloud resolve",
            &[
                gpu::texture_2d_entry(0),
                gpu::texture_entry(1, D::D2, false),
                gpu::texture_2d_entry(2),
                gpu::texture_2d_entry(3),
                gpu::texture_2d_entry(4),
                gpu::storage_2d_entry(5, half),
                gpu::storage_2d_entry(6, half),
            ],
        );
        let composite_layout = gpu::layout(
            device,
            "cloud composite",
            &[
                gpu::texture_entry(0, D::D2, false),
                gpu::texture_entry(1, D::D2, false),
            ],
        );
        let composite_depth_layout = gpu::layout(
            device,
            "cloud composite depth",
            &[gpu::depth_entry(0), gpu::texture_2d_entry(1)],
        );
        let composite_depth = (
            0,
            gpu::bind_group(
                device,
                "cloud composite depth",
                &composite_depth_layout,
                &[
                    (0, wgpu::BindingResource::TextureView(&empty_depth.view)),
                    (1, wgpu::BindingResource::TextureView(&underwater_probe)),
                ],
            ),
        );
        let shadow_interface_layout = gpu::layout(
            device,
            "cloud shadow",
            &[
                gpu::texture_2d_entry(0),
                gpu::sampler_entry(1),
                gpu::uniform_entry(2),
            ],
        );

        let shadow_field_group = gpu::bind_group(
            device,
            "cloud field",
            &shadow_field_layout,
            &field_resources(
                &buffer,
                &weather,
                &shape,
                &detail,
                &turbulence,
                &blue_noise,
                &samplers,
            ),
        );
        let uniforms_group = gpu::bind_group(
            device,
            "cloud uniforms",
            &uniforms_layout,
            &[
                (0, buffer.as_entire_binding()),
                (7, R::Sampler(&samplers[0])),
                (8, R::Sampler(&samplers[1])),
            ],
        );
        let shadow_out_group = gpu::bind_group(
            device,
            "cloud shadow maps out",
            &shadow_out_layout,
            &[
                (0, R::TextureView(&shadow_targets.current.view)),
                (1, R::TextureView(&shadow_targets.depth_velocity.view)),
            ],
        );
        let shadow_resolve_groups =
            shadow_resolve_groups(device, &shadow_resolve_layout, &shadow_targets);
        let ground_out_group = gpu::bind_group(
            device,
            "cloud ground shadow out",
            &ground_out_layout,
            &[(0, R::TextureView(&ground_map.view))],
        );

        // Pipelines.
        let sky_layout = sky.bind_group_layout();
        let compute = |source: &str,
                       path: &str,
                       entry: &str,
                       layouts: &[Option<&wgpu::BindGroupLayout>]| {
            make.compute(|| {
                let module = shaders::shader_module(device, source, path, &[]);
                // The shader's path names its module; the pipeline takes a label DX12 can use as a file name.
                gpu::compute_pipeline(device, &format!("clouds {entry}"), &module, entry, layouts)
            })
        };
        let shadow_pipeline = compute(
            shaders::SHADOW_PASS_WGSL,
            shaders::SHADOW_PASS_PATH,
            "shadow_main",
            &[
                None,
                None,
                Some(&shadow_field_layout),
                Some(&shadow_out_layout),
            ],
        );
        let shadow_resolve_pipeline = compute(
            shaders::SHADOW_RESOLVE_WGSL,
            shaders::SHADOW_RESOLVE_PATH,
            "shadow_resolve_main",
            &[
                None,
                None,
                Some(&uniforms_layout),
                Some(&shadow_resolve_layout),
            ],
        );
        let ground_pipeline = compute(
            shaders::GROUND_WGSL,
            shaders::GROUND_PATH,
            "ground_main",
            &[
                None,
                None,
                Some(&shadow_field_layout),
                Some(&ground_out_layout),
            ],
        );
        let march_pipeline = compute(
            shaders::MARCH_PASS_WGSL,
            shaders::MARCH_PASS_PATH,
            "march_main",
            &[
                None,
                Some(sky_layout),
                Some(&march_layout),
                Some(&march_out_layout),
            ],
        );
        let march_full_pipeline = compute(
            shaders::MARCH_PASS_WGSL,
            shaders::MARCH_PASS_PATH,
            "march_full_main",
            &[
                None,
                Some(sky_layout),
                Some(&march_layout),
                Some(&march_out_layout),
            ],
        );
        let resolve_pipeline = compute(
            shaders::RESOLVE_WGSL,
            shaders::RESOLVE_PATH,
            "resolve_main",
            &[
                None,
                Some(&composite_depth_layout),
                Some(&uniforms_layout),
                Some(&resolve_layout),
            ],
        );
        let composite_pipeline = make.render(|| {
            let module = shaders::shader_module(
                device,
                shaders::COMPOSITE_WGSL,
                shaders::COMPOSITE_PATH,
                &[],
            );
            composite_pipeline(
                device,
                &module,
                &[&composite_layout, &uniforms_layout, &composite_depth_layout],
                targets,
                None,
            )
        });

        let shadow_uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cloud shadow uniforms"),
            size: std::mem::size_of::<CloudShadowUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_interface = make_shadow_interface(
            device,
            &shadow_interface_layout,
            &ground_map,
            &samplers[0],
            shadow_uniforms,
        );

        let (field_group, march_out_group, march_full_out_groups, resolve_groups, composite_groups) =
            screen_groups(
                device,
                &field_layout,
                &march_out_layout,
                &resolve_layout,
                &composite_layout,
                &screen,
                (
                    &buffer,
                    &weather,
                    &shape,
                    &detail,
                    &turbulence,
                    &blue_noise,
                    &samplers,
                ),
            );

        let mut layer = Self {
            quality,
            render_scale: 1.0,
            tuning: CloudTuning::default(),
            tier,
            layers: Layers::takram(0.3),
            coverage: 0.3,
            clearing: None,
            uniforms: CloudUniforms::zeroed(),
            buffer,
            weather,
            shape,
            detail,
            turbulence,
            blue_noise,
            turbulence_fill,
            detail_fill,
            shadow_targets,
            ground_map,
            screen,
            empty_depth,
            underwater_probe,
            no_probe,
            samplers,
            field_layout,
            field_group,
            field_changed: true,
            shadow_field_layout,
            shadow_field_group,
            march_layout,
            march_groups: None,
            uniforms_layout,
            uniforms_group,
            shadow_out_layout,
            shadow_out_group,
            shadow_resolve_layout,
            shadow_resolve_groups,
            ground_out_layout,
            ground_out_group,
            march_out_layout,
            march_out_group,
            march_full_out_groups,
            resolve_layout,
            resolve_groups,
            composite_layout,
            composite_groups,
            composite_depth_layout,
            composite_depth,
            shadow_interface,
            shadow_interface_layout,
            shadow_pipeline,
            shadow_resolve_pipeline,
            ground_pipeline,
            march_pipeline,
            march_full_pipeline,
            resolve_pipeline,
            composite_pipeline,
            targets,
            width: 0,
            height: 0,
            updates: 0,
            current: 0,
            has_history: false,
            cover: naval_sky::CloudCover::default(),
            screens_made: 1,
            shadow_latest: 0,
            shadow_has_history: false,
            ground_stale: true,
            ground_night: false,
            ground_direction: Vec3::Y,
            moon_key: false,
            flash_intensity: 0.0,
            flash_position: [0.0; 3],
            ground_next: 0,
            active: true,
            held: false,
            held_updates: 0,
            held_view: Mat4::ZERO,
            still: false,
            time_stands: true,
            still_updates: 0,
            still_view: Mat4::ZERO,
            previous_view_projection: Mat4::IDENTITY,
            previous_drift: Vec3::ZERO,
            drawn_shadow_matrices: [Mat4::IDENTITY; MAX_CASCADES],
            drawn_inverse_shadow_matrices: [Mat4::IDENTITY; MAX_CASCADES],
            drawn_cascades: [Cascade::default(); MAX_CASCADES],
            shadow_rounds: 0,
            plan: Plan::default(),
            timestamps: None,
        };
        layer.write_static_uniforms();
        layer
    }

    /// Builds the layer's pipelines through the renderer's registry, as [`CloudLayer::new`] built them, for
    /// [`CloudLayer::adopt_pipelines`]. The sky's and the clouds' files must be registered first
    /// (`naval_sky::shaders::register`, [`shaders::register`]).
    pub fn register_pipelines(
        &self,
        ctx: &mut RenderContext<'_>,
        sky: &Sky,
    ) -> Result<CloudPipelines, ShaderError> {
        let none = ctx
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("cloud no group"),
                entries: &[],
            });
        let sky_layout = sky.bind_group_layout().clone();
        let mut add =
            |label: &str, path: &str, layouts: Vec<wgpu::BindGroupLayout>, entry: &'static str| {
                ctx.add_pipeline(label, path, Vec::new(), compute_recipe(layouts, entry))
            };
        let shadow = add(
            "cloud shadow maps",
            shaders::SHADOW_PASS_PATH,
            vec![
                none.clone(),
                none.clone(),
                self.shadow_field_layout.clone(),
                self.shadow_out_layout.clone(),
            ],
            "shadow_main",
        )?;
        let shadow_resolve = add(
            "cloud shadow maps resolve",
            shaders::SHADOW_RESOLVE_PATH,
            vec![
                none.clone(),
                none.clone(),
                self.uniforms_layout.clone(),
                self.shadow_resolve_layout.clone(),
            ],
            "shadow_resolve_main",
        )?;
        let ground = add(
            "cloud ground shadow",
            shaders::GROUND_PATH,
            vec![
                none.clone(),
                none.clone(),
                self.shadow_field_layout.clone(),
                self.ground_out_layout.clone(),
            ],
            "ground_main",
        )?;
        let march = add(
            "cloud march",
            shaders::MARCH_PASS_PATH,
            vec![
                none.clone(),
                sky_layout.clone(),
                self.march_layout.clone(),
                self.march_out_layout.clone(),
            ],
            "march_main",
        )?;
        let march_full = add(
            "cloud full march",
            shaders::MARCH_PASS_PATH,
            vec![
                none.clone(),
                sky_layout,
                self.march_layout.clone(),
                self.march_out_layout.clone(),
            ],
            "march_full_main",
        )?;
        let resolve = add(
            "cloud resolve",
            shaders::RESOLVE_PATH,
            vec![
                none,
                self.composite_depth_layout.clone(),
                self.uniforms_layout.clone(),
                self.resolve_layout.clone(),
            ],
            "resolve_main",
        )?;
        let turbulence = add(
            "cloud turbulence",
            shaders::NOISE_PATH,
            vec![self.turbulence_fill.layout.clone()],
            "turbulence_main",
        )?;
        let detail = self.detail_fill.register(ctx)?;
        let layouts = [
            self.composite_layout.clone(),
            self.uniforms_layout.clone(),
            self.composite_depth_layout.clone(),
        ];
        let targets = self.targets;
        let recipe: PipelineRecipe = Box::new(move |build| {
            Pipeline::Render(composite_pipeline(
                build.device,
                build.module,
                &[&layouts[0], &layouts[1], &layouts[2]],
                targets,
                build.cache,
            ))
        });
        let composite = ctx.add_pipeline(
            "cloud composite",
            shaders::COMPOSITE_PATH,
            Vec::new(),
            recipe,
        )?;
        Ok(CloudPipelines {
            shadow,
            shadow_resolve,
            ground,
            march,
            march_full,
            resolve,
            composite,
            turbulence,
            detail,
        })
    }

    /// Takes the registry's current pipelines (rebuilt after a shader edit) in place of the layer's own. Returns
    /// whether any changed: the host then drops the histories (a cut), so the edit shows at once.
    pub fn adopt_pipelines(&mut self, registry: &PipelineRegistry, ids: &CloudPipelines) -> bool {
        fn take<T: Clone + PartialEq>(slot: &mut T, new: &T) -> bool {
            let changed = slot != new;
            if changed {
                *slot = new.clone();
            }
            changed
        }
        let mut changed = take(&mut self.shadow_pipeline, registry.compute(ids.shadow))
            | take(
                &mut self.shadow_resolve_pipeline,
                registry.compute(ids.shadow_resolve),
            )
            | take(&mut self.ground_pipeline, registry.compute(ids.ground))
            | take(&mut self.march_pipeline, registry.compute(ids.march))
            | take(
                &mut self.march_full_pipeline,
                registry.compute(ids.march_full),
            )
            | take(&mut self.resolve_pipeline, registry.compute(ids.resolve))
            | take(&mut self.composite_pipeline, registry.render(ids.composite));
        // A rebuilt fill fills the turbulence again at the next prepare (`encode_noise_fill`).
        if take(
            &mut self.turbulence_fill.pipeline,
            registry.compute(ids.turbulence),
        ) {
            self.turbulence_fill.filled = false;
            changed = true;
        }
        // And the detail noise.
        changed |= self.detail_fill.adopt(registry, &ids.detail);
        if changed {
            self.ground_stale = true;
        }
        changed
    }

    /// Fills the turbulence and the detail noise on `encoder` when they do not hold the current fills' output yet (a
    /// layer built with [`CloudLayer::new_deferred`], or `noise.wgsl` or `detail_noise.wgsl` rebuilt). Before the
    /// layer's other passes.
    pub fn encode_noise_fill(&mut self, encoder: &mut wgpu::CommandEncoder) {
        if !self.turbulence_fill.filled {
            self.turbulence_fill.encode(encoder);
        }
        self.detail_fill.encode_if_stale(encoder);
    }

    /// What the sky needs to show the clouds in its dome and environment (`Sky::attach_clouds`).
    pub fn hooks(&self) -> CloudHooks {
        CloudHooks {
            modules: shaders::modules(),
            bind_group_layout: self.field_layout.clone(),
            bind_group: self.field_group.clone(),
        }
    }

    /// The hooks' group when it changed since last asked (the reconstruction's targets were remade), for
    /// `Sky::set_cloud_bind_group`.
    pub fn take_field_group(&mut self) -> Option<wgpu::BindGroup> {
        std::mem::take(&mut self.field_changed).then(|| self.field_group.clone())
    }

    /// Clears the clouds' shadow about a point and nowhere else (the port's berth, `PORT_CLEARING`), or stops
    /// (`None`). The clouds themselves still cross the sky; only the shadow ships, islands and the sea read clears.
    pub fn set_clearing(&mut self, clearing: Option<CloudClearing>) {
        self.clearing = clearing;
    }

    /// The clearing in the clouds' shadow, if any ([`CloudLayer::set_clearing`]).
    pub fn clearing(&self) -> Option<CloudClearing> {
        self.clearing
    }

    /// The cloud shadow for ships, islands and the sea.
    pub fn shadow(&self) -> &CloudShadow {
        &self.shadow_interface
    }

    pub fn quality(&self) -> SkyQuality {
        self.quality
    }

    pub fn tier(&self) -> CloudTier {
        self.tier
    }

    pub fn uniforms(&self) -> &CloudUniforms {
        &self.uniforms
    }

    /// The layers drawn: the weather's (`crate::weathers`), or between two weathers' while it changes.
    pub fn layers(&self) -> &Layers {
        &self.layers
    }

    /// Diagnostics: other layers (the next prepare takes them; the next scene applied replaces them).
    pub fn set_layers(&mut self, layers: Layers) {
        self.layers = layers;
        self.write_layer_uniforms();
        self.drop_history();
    }

    fn drop_history(&mut self) {
        self.has_history = false;
        self.shadow_has_history = false;
        self.ground_stale = true;
    }

    /// Diagnostics: draw clouds at all (the bake, shafts and shadows follow).
    pub fn set_enabled(&mut self, enabled: bool) {
        self.active = enabled;
        self.has_history = false;
        self.shadow_has_history = false;
        self.ground_stale = true;
    }

    pub fn enabled(&self) -> bool {
        self.active
    }

    /// The high cloud the console reads (amount, altitude in m): the high layer's coverage and base (takram's high
    /// layer in fair weather, at the sky's cover).
    pub fn high(&self) -> (f64, f64) {
        let high = self.layers.0[2];
        (f64::from(high.coverage), f64::from(high.altitude))
    }

    /// See `SkyApi.hold`: restart the march's sequence, and stop marching once a still view has converged.
    pub fn hold(&mut self, held: bool) {
        self.held = held;
        self.held_updates = 0;
        self.updates = 0;
    }

    /// The air in front of the clouds changed (the aerial perspective's haze, which the march takes at the clouds'
    /// front): a held or paused picture that had converged marches again from its history until it converges on it.
    pub fn air_changed(&mut self) {
        self.held_updates = 0;
        self.still_updates = 0;
    }

    /// Updates a held view has marched (48 is converged).
    pub fn held_updates(&self) -> u32 {
        self.held_updates
    }

    /// Whether this frame's time stands still while frames go on (a paused picture). Under a still view the march then
    /// converges as a held view's does and stops, so every paused frame draws the same clouds to the bit; a moving
    /// camera marches on from its history, and the march goes on as before once time runs again.
    pub fn set_still(&mut self, still: bool) {
        self.still = still;
    }

    /// Whether this frame leaves time where it was (a capture, a frame drawn again, a paused picture, the first frame).
    /// The first update after a cut on such a frame marches every pixel, so the picture it shows is whole; in play a cut
    /// fills in over the next 16 updates, as takram's does, rather than spend one long frame.
    pub fn set_time_stands(&mut self, stands: bool) {
        self.time_stands = stands;
    }

    /// Updates marched under a still view while time stood still (48 is converged and the march stops).
    pub fn still_updates(&self) -> u32 {
        self.still_updates
    }

    /// Whether the last update marched every pixel (the first after a cut while time stood: `set_time_stands`).
    pub fn marched_whole(&self) -> bool {
        self.plan.update && self.plan.full
    }

    /// Profiling: a query set of at least ten timestamps the passes write: 0–1 the shadow maps, 2–3 their temporal
    /// pass, 4–5 the ground shadow, 6–7 the march, 8–9 the reconstruction. A pass that does not run writes none.
    pub fn set_timestamps(&mut self, query_set: Option<wgpu::QuerySet>) {
        self.timestamps = query_set;
    }

    /// Apply a scene at once (`CloudsEffect`'s coverage): the sky's cover and the weather's layers
    /// (`crate::weathers::layers`: takram's in fair weather, a deck, a storm, fog banks, a genus at the base and depth the
    /// scene chose). New clouds drop the histories, as a new scene does.
    pub fn apply(&mut self, _queue: &wgpu::Queue, _sky: &mut Sky, scene: &SkyScene) {
        let coverage = scene.clouds.coverage.clamp(0.0, 1.0) as f32;
        let layers = crate::weathers::layers(scene, self.tier.rain_shafts);
        if coverage != self.coverage || layers != self.layers {
            self.coverage = coverage;
            self.layers = layers;
            self.write_layer_uniforms();
            self.drop_history();
        }
        self.uniforms.medium[2] = self.coverage;
    }

    /// The clouds `t` of the way from `from`'s weather to `to`'s (0 `from`, 1 `to`), layer parameter by parameter
    /// ([`Layers::lerp`]): a weather changing as the sky drifts, so every history is kept and the march, the shadow maps
    /// and the ground map follow the clouds as they follow drifting ones.
    /// `from_layers` stands in for `from`'s weather's layers: the layers on screen when a change interrupted another.
    pub fn blend(&mut self, from: &SkyScene, from_layers: Option<&Layers>, to: &SkyScene, t: f32) {
        let t = t.clamp(0.0, 1.0);
        let coverage = |scene: &SkyScene| scene.clouds.coverage.clamp(0.0, 1.0) as f32;
        self.coverage = coverage(from) + (coverage(to) - coverage(from)) * t;
        let shafts = self.tier.rain_shafts;
        let start = from_layers
            .copied()
            .unwrap_or_else(|| crate::weathers::layers(from, shafts));
        self.layers = start.lerp(&crate::weathers::layers(to, shafts), t);
        self.write_layer_uniforms();
        self.uniforms.medium[2] = self.coverage;
    }

    /// Graphics → Clouds: takram's preset for the tier, its shadow maps' size and count, the ground map's size.
    pub fn set_quality(&mut self, device: &wgpu::Device, quality: SkyQuality) {
        use wgpu::BindingResource as R;
        self.quality = quality;
        let previous = self.tier;
        self.tier = quality::tier(quality);
        if (previous.shadow.map_size, previous.shadow.cascade_count)
            != (self.tier.shadow.map_size, self.tier.shadow.cascade_count)
        {
            self.shadow_targets = ShadowTargets::new(
                device,
                self.tier.shadow.map_size,
                self.tier.shadow.cascade_count,
            );
            self.shadow_out_group = gpu::bind_group(
                device,
                "cloud shadow maps out",
                &self.shadow_out_layout,
                &[
                    (0, R::TextureView(&self.shadow_targets.current.view)),
                    (1, R::TextureView(&self.shadow_targets.depth_velocity.view)),
                ],
            );
            self.shadow_resolve_groups =
                shadow_resolve_groups(device, &self.shadow_resolve_layout, &self.shadow_targets);
            self.march_groups = None;
            self.shadow_has_history = false;
        }
        if previous.ground_size != self.tier.ground_size {
            self.ground_map = gpu::target(
                device,
                "cloud ground shadow",
                self.tier.ground_size,
                self.tier.ground_size,
                wgpu::TextureFormat::Rgba8Unorm,
            );
            self.ground_out_group = gpu::bind_group(
                device,
                "cloud ground shadow out",
                &self.ground_out_layout,
                &[(0, R::TextureView(&self.ground_map.view))],
            );
            let uniforms = self.shadow_interface.uniforms.clone();
            self.shadow_interface = make_shadow_interface(
                device,
                &self.shadow_interface_layout,
                &self.ground_map,
                &self.samplers[0],
                uniforms,
            );
        }
        self.ground_stale = true;
        self.has_history = false;
        self.write_static_uniforms();
    }

    /// Scale the cloud history and reconstruction, independently of the quality tier.
    /// Targets and history are replaced only when prepare observes a new pixel size.
    pub fn set_render_scale(&mut self, scale: f32) {
        self.render_scale = if scale.is_finite() { scale.clamp(0.25, 1.0) } else { 1.0 };
    }

    /// Live controls use the existing uniforms/targets. Switching drops contaminated histories once.
    pub fn set_tuning(&mut self, tuning: CloudTuning) {
        let tuning = tuning.sanitize();
        if self.tuning != tuning {
            self.tuning = tuning;
            self.write_tuning_uniforms();
            self.drop_history();
        }
    }

    fn write_tuning_uniforms(&mut self) {
        self.uniforms.experiments = [self.tuning.debug as f32, self.tuning.shadow_mode as f32,
            self.tuning.far_step_scale, if self.tuning.sun_steps == 0 { 1.0 } else { 0.0 }];
        self.uniforms.secondary[0] = if self.tuning.sun_steps < 0 {
            self.tier.clouds.max_iteration_count_to_sun as f32
        } else { self.tuning.sun_steps as f32 };
    }

    /// The uniforms that change only with the tier or the layers.
    fn write_static_uniforms(&mut self) {
        let tier = self.tier;
        let march = tier.clouds;
        let shadow = tier.shadow;
        let flag = |on: bool| if on { 1.0 } else { 0.0 };
        let u = &mut self.uniforms;
        u.medium = [
            SCATTERING_COEFFICIENT,
            ABSORPTION_COEFFICIENT,
            self.coverage,
            TURBULENCE_DISPLACEMENT,
        ];
        u.repeats = [
            1.0 / WEATHER_TILE,
            SHAPE_REPEAT,
            SHAPE_DETAIL_REPEAT,
            TURBULENCE_REPEAT,
        ];
        u.scattering = [
            SKY_LIGHT_SCALE,
            GROUND_BOUNCE_SCALE,
            POWDER_SCALE,
            POWDER_EXPONENT,
        ];
        u.phase = [
            SCATTER_ANISOTROPY[0],
            SCATTER_ANISOTROPY[1],
            SCATTER_ANISOTROPY[2],
            march.multi_scattering_octaves as f32,
        ];
        u.march = [
            march.max_iteration_count as f32,
            march.min_step_size,
            march.max_step_size,
            march.max_ray_distance,
        ];
        u.march_limits = [
            march.perspective_step_scale,
            march.min_density,
            march.min_extinction,
            march.min_transmittance,
        ];
        u.secondary = [
            march.max_iteration_count_to_sun as f32,
            march.max_iteration_count_to_ground as f32,
            march.min_secondary_step_size,
            march.secondary_step_scale,
        ];
        u.shafts = [
            march.max_shadow_length_iteration_count as f32,
            march.min_shadow_length_step_size,
            march.max_shadow_length_ray_distance,
            flag(tier.light_shafts),
        ];
        u.haze = HAZE;
        u.switches = [
            flag(tier.shape_detail),
            flag(tier.turbulence),
            flag(tier.haze),
            flag(march.accurate_sun_sky_light),
        ];
        // takram defines GROUND_BOUNCE with a bounce scale and marches toward the ground (here every tier takes the
        // bounce, marched or not: `shaders/march.wgsl`, `radiance_from_ground`), POWDER with a powder scale.
        u.state[0] = flag(GROUND_BOUNCE_SCALE > 0.0);
        u.state[1] = flag(POWDER_SCALE > 0.0);
        u.shadow_map = [
            shadow.cascade_count as f32,
            shadow.map_size as f32,
            MAX_SHADOW_FILTER_RADIUS,
            SHADOW_SAMPLE_COUNT,
        ];
        u.shadow_march = [
            shadow.max_iteration_count as f32,
            shadow.min_step_size,
            shadow.max_step_size,
            shadow.min_density,
        ];
        u.shadow_limits[0] = shadow.min_extinction;
        u.shadow_limits[1] = shadow.min_transmittance;
        u.shadow_limits[2] = OPTICAL_DEPTH_TAIL_SCALE;
        u.ground[0] = tier.ground_size as f32;
        u.ground[3] = tier.ground_steps as f32;
        u.bake[0] = tier.environment_steps as f32;
        self.write_layer_uniforms();
        self.write_tuning_uniforms();
    }

    fn write_layer_uniforms(&mut self) {
        let packed = self.layers.pack();
        let u = &mut self.uniforms;
        u.min_layer_heights = packed.min_layer_heights;
        u.max_layer_heights = packed.max_layer_heights;
        let [a, b, c] = packed.min_interval_heights;
        u.min_interval_heights = [a, b, c, 0.0];
        let [a, b, c] = packed.max_interval_heights;
        u.max_interval_heights = [a, b, c, 0.0];
        u.density_scales = packed.density_scales;
        u.shape_amounts = packed.shape_amounts;
        u.shape_detail_amounts = packed.shape_detail_amounts;
        u.weather_exponents = packed.weather_exponents;
        u.shape_altering_biases = packed.shape_altering_biases;
        u.coverage_filter_widths = packed.coverage_filter_widths;
        u.layer_coverages = packed.coverages;
        u.layer_slabs = packed.slabs;
        u.heights = [
            packed.min_height,
            packed.max_height,
            packed.shadow_top_height,
            packed.shadow_bottom_height,
        ];
        u.shadow_layer_mask = packed.shadow_layer_mask;
        u.density_exp_terms = packed.exp_terms;
        u.density_exponents = packed.exponents;
        u.density_linear_terms = packed.linear_terms;
        u.density_constant_terms = packed.constant_terms;
        u.channels = packed.channels;
    }

    fn resize(&mut self, device: &wgpu::Device) {
        self.screen = ScreenTargets::new(device, self.width, self.height);
        let (field_group, march_out_group, march_full_out_groups, resolve_groups, composite_groups) =
            screen_groups(
                device,
                &self.field_layout,
                &self.march_out_layout,
                &self.resolve_layout,
                &self.composite_layout,
                &self.screen,
                (
                    &self.buffer,
                    &self.weather,
                    &self.shape,
                    &self.detail,
                    &self.turbulence,
                    &self.blue_noise,
                    &self.samplers,
                ),
            );
        self.field_group = field_group;
        self.field_changed = true;
        self.march_out_group = march_out_group;
        self.march_full_out_groups = march_full_out_groups;
        self.resolve_groups = resolve_groups;
        self.composite_groups = composite_groups;
        self.has_history = false;
        self.current = 0;
        self.screens_made += 1;
    }

    /// The CPU half of a frame (`CloudsEffect.update`): the cascades, the uniforms and what this update runs; `cut`
    /// drops the histories (a camera jump or a new scene).
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &mut naval_render::UploadSink<'_>,
        view: &CloudView,
        sky: &Sky,
        cut: bool,
    ) {
        let viewport = view.viewport.map(|size| size.max(1));
        let [width, height] = viewport.map(|size| (size as f32 * self.render_scale).ceil().max(1.0) as u32);
        if width.max(1) != self.width || height.max(1) != self.height {
            self.width = width.max(1);
            self.height = height.max(1);
            self.resize(device);
        }
        let view_projection = view.view_projection();
        // The clouds' key light: the sun, or the moon once its beam outshines the sun's at the top of the clouds. The
        // march, the Beer shadow maps, the shafts and the bake follow it; a change of body drops the maps' history.
        let key = crate::key_light::KeyLight::of(sky, f64::from(self.uniforms.heights[1]));
        let sun = key.direction;
        if key.moon != self.moon_key {
            self.moon_key = key.moon;
            self.shadow_has_history = false;
            self.shadow_rounds = 0;
            self.held_updates = 0;
            self.still_updates = 0;
        }
        // A lightning flash lights the clouds while it lasts: the march goes on (a held or paused picture too) and the
        // reconstruction follows it, until an update after it has gone.
        let flash = sky.uniforms().lightning_intensity;
        let flash_position = sky.uniforms().lightning_position;
        let flashing = flash > 0.0 || self.flash_intensity > 0.0;
        if flash != self.flash_intensity || (flash > 0.0 && flash_position != self.flash_position) {
            self.held_updates = 0;
            self.still_updates = 0;
        }
        self.flash_intensity = flash;
        self.flash_position = flash_position;
        // The cloud shadow stops the light the scene is lit by: the moon's once it outshines the sun, so an overcast
        // night has no moonlit hulls under it. A change of body, or the light turning since the map was last marched
        // (in play the sweep follows it update by update; a held or paused picture that had converged has stopped, and
        // no cut comes with, say, a new moon phase), marches the map whole, the count of updates starting again.
        let light = sky.light();
        let shadow_light = light.direction.normalize_or(Vec3::Y);
        if light.night != self.ground_night
            || shadow_light.angle_between(self.ground_direction) > GROUND_TURN
        {
            self.ground_night = light.night;
            self.ground_stale = true;
            self.held_updates = 0;
            self.still_updates = 0;
        }
        let wind = sky.wind_offset();
        let camera_world = view.view.inverse();
        let forward = camera_world.transform_vector3(Vec3::NEG_Z).normalize();

        if cut {
            self.has_history = false;
            self.shadow_has_history = false;
            self.shadow_rounds = 0;
            self.ground_stale = true;
        }
        // Held, every new view starts afresh and marches the same sequence from its start, then stands still.
        if self.held && (cut || !self.has_history || !same_view(&view_projection, &self.held_view))
        {
            self.held_view = view_projection;
            self.held_updates = 0;
            self.updates = 0;
            self.has_history = false;
            self.shadow_has_history = false;
            self.shadow_rounds = 0;
            self.ground_stale = true;
            self.current = 0;
            self.shadow_latest = 0;
        }
        // A paused picture: time standing still under the same view converges like a held view (keeping its history)
        // and then stops marching; a new view, a cut or time running starts the count again.
        if !(self.still
            && !cut
            && self.has_history
            && same_view(&view_projection, &self.still_view))
        {
            self.still_view = view_projection;
            self.still_updates = 0;
        }
        let converged =
            (self.held && self.held_updates >= HOLD_UPDATES) || self.still_updates >= HOLD_UPDATES;

        let fit_view = self.tuning.shadow_view(view);
        let cascades = Cascades::fit(
            &CascadeSettings {
                count: self.tier.shadow.cascade_count,
                map_size: self.tier.shadow.map_size,
                reach_far: self.uniforms.march[3].max(self.uniforms.shafts[2]),
                // A theatre view drawing the land beyond its window reaches far past a battle view's 60 km
                // (`HORIZON_FAR`): the cascades keep a battle view's span, which spread over its far plane would lose
                // their resolution about the camera. The last still serves out to `reach_far`.
                max_far: (self.tuning.shadow_far == 0.0 && view.far > naval_render::camera::DEFAULT_FAR
                    && view.far <= naval_render::camera::HORIZON_FAR)
                    .then_some(naval_render::camera::DEFAULT_FAR),
                ..CascadeSettings::default()
            },
            camera_world,
            fit_view.projection,
            fit_view.near,
            fit_view.far,
            sun,
            light_distance(sun.y.clamp(0.0, 1.0)),
        );

        let low = [width.max(1).div_ceil(4), height.max(1).div_ceil(4)];
        {
            let u = &mut self.uniforms;
            u.view = view.view.to_cols_array_2d();
            u.camera = [
                view.position.x,
                view.position.y,
                view.position.z,
                view.position.y,
            ];
            u.forward = [forward.x, forward.y, forward.z, 1.0 / view.tan_half_fov()];
            u.screen = [
                self.width as f32,
                self.height as f32,
                low[0] as f32,
                low[1] as f32,
            ];
            u.viewport = [viewport[0] as f32, viewport[1] as f32, 0.0, 0.0];
            u.planes = [view.near, view.far, cascades.far, MIP_LEVEL_SCALE];
            u.sun = [sun.x, sun.y, sun.z, PLANET_RADIUS];
            let drift = Vec3::new(wind.x, 0.0, wind.z);
            // takram shifts the shape by the weather's offset (in tiles) × 2e4 m.
            let evolution = drift.length() / WEATHER_TILE * 2e4;
            u.drift = [drift.x, drift.y, drift.z, evolution];
            u.medium[2] = self.coverage;
            for (index, cascade) in cascades.cascades.iter().enumerate() {
                u.shadow_intervals[index] = [cascade.interval[0], cascade.interval[1], 0.0, 0.0];
            }
            // Each cascade is sampled as it was last marched (the update below re-marches one, or all after a cut).
            for index in 0..MAX_CASCADES {
                u.shadow_matrices[index] = self.drawn_shadow_matrices[index].to_cols_array_2d();
                u.inverse_shadow_matrices[index] =
                    self.drawn_inverse_shadow_matrices[index].to_cols_array_2d();
            }
            u.bake[1] = smoothstep(SHADOW_SUN[0], SHADOW_SUN[1], f64::from(shadow_light.y)) as f32;
            u.ground_light = [shadow_light.x, shadow_light.y, shadow_light.z, 0.0];
            u.key_light = key.uniform();
            u.flash = [if flashing { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0];
            // The dome takes the shafts' length out of the sun's light: under the moon there are none.
            u.shafts[3] = if self.tier.light_shafts && !key.moon {
                1.0
            } else {
                0.0
            };
            u.state[2] = if self.active { 1.0 } else { 0.0 };
        }

        self.plan = Plan {
            screen_size: [self.width, self.height],
            march_size: low,
            ..Plan::default()
        };
        if !self.active || converged {
            self.uniforms.state[3] = self.current as f32;
            queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&self.uniforms));
            self.write_shadow_interface(queue, sky, view);
            return;
        }

        // This update's cell, the histories it reads and writes.
        let cell = bayer_cell(self.updates);
        let shadow_from = self.shadow_latest;
        let clouds_from = self.current;
        self.ground_direction = shadow_light;
        let ground = if self.ground_stale {
            self.ground_stale = false;
            self.ground_next = 0;
            Some((0, self.tier.ground_size * self.tier.ground_size))
        } else {
            let total = self.tier.ground_size * self.tier.ground_size;
            let first = self.ground_next % total;
            let count = GROUND_TEXELS_PER_UPDATE.min(total - first);
            self.ground_next = (first + count) % total;
            Some((first, count))
        };
        {
            let u = &mut self.uniforms;
            u.frame = [
                (self.updates % 4096) as f32,
                cell[0] as f32,
                cell[1] as f32,
                if self.has_history { 1.0 } else { 0.0 },
            ];
            u.previous_view_projection = if self.has_history {
                self.previous_view_projection
            } else {
                view_projection
            }
            .to_cols_array_2d();
            // How far the wind carried the clouds since the history was marched: the march's motion follows a
            // drifting cloud with it.
            let drift = Vec3::new(wind.x, 0.0, wind.z);
            let step = if self.has_history {
                drift - self.previous_drift
            } else {
                Vec3::ZERO
            };
            u.drift_step = [step.x, step.y, step.z, 0.0];
            self.previous_drift = drift;
            // One cascade an update, round robin; every cascade after a cut or a fresh start, whose history is gone.
            // The shadow maps' temporal pass blends a march in at 1% (takram's `temporalAlpha` 0.01), so a cascade
            // marched a third as often converges a third as fast but looks the same; the march costs a third.
            let count = self.tier.shadow.cascade_count.max(1) as u32;
            let all = (1u32 << count) - 1;
            // A turn or a move under the cut's thresholds can carry the view out of a cascade marched for an earlier
            // one (a cascade samples nothing outside its square); that cascade is marched again too.
            let next = self.shadow_rounds % count;
            let mask = if self.shadow_has_history {
                (0..count as usize).fold(1u32 << next, |mask, index| {
                    if cascades.cascades[index].escapes(&self.drawn_cascades[index]) {
                        mask | 1 << index
                    } else {
                        mask
                    }
                })
            } else {
                all
            };
            self.shadow_rounds = self.shadow_rounds.wrapping_add(1);
            for (index, cascade) in cascades.cascades.iter().enumerate() {
                if mask >> index & 1 == 0 {
                    continue;
                }
                // The march reprojects from where this cascade was last marched.
                u.shadow_reprojection[index] = if self.shadow_has_history {
                    self.drawn_shadow_matrices[index]
                } else {
                    cascade.matrix
                }
                .to_cols_array_2d();
                self.drawn_shadow_matrices[index] = cascade.matrix;
                self.drawn_inverse_shadow_matrices[index] = cascade.inverse_matrix;
                self.drawn_cascades[index] = *cascade;
                u.shadow_matrices[index] = cascade.matrix.to_cols_array_2d();
                u.inverse_shadow_matrices[index] = cascade.inverse_matrix.to_cols_array_2d();
            }
            u.cascades = [mask as f32, mask.count_ones() as f32, 0.0, 0.0];
            u.shadow_limits[3] = if self.shadow_has_history { 1.0 } else { 0.0 };
            if let Some((first, count)) = ground {
                u.ground[1] = first as f32;
                u.ground[2] = count as f32;
            }
            // The reconstruction writes the other history, which the dome then reads for the shafts.
            u.state[3] = (1 - clouds_from) as f32;
        }
        self.plan = Plan {
            update: true,
            shadow_from,
            shadow_count: self.uniforms.cascades[1] as u32,
            clouds_from,
            ground,
            full: !self.has_history && self.time_stands,
            march_size: low,
            screen_size: [self.width, self.height],
        };
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&self.uniforms));
        self.write_shadow_interface(queue, sky, view);

        // From here on, this update's results are the latest.
        self.updates = self.updates.wrapping_add(1);
        if self.held {
            self.held_updates += 1;
        }
        if self.still {
            self.still_updates += 1;
        }
        self.current = 1 - clouds_from;
        self.shadow_latest = 1 - shadow_from;
        self.has_history = true;
        self.shadow_has_history = true;
        self.previous_view_projection = view_projection;
    }

    /// Refresh just the shared shadow uniforms. Hosts with clouds disabled can
    /// keep a valid clear-sky interface without allocating screen targets or
    /// compiling/dispatching the cloud pipelines.
    pub fn write_shadow_interface(
        &self,
        queue: &mut naval_render::UploadSink<'_>,
        sky: &Sky,
        view: &CloudView,
    ) {
        let wind = sky.wind_offset();
        let uniforms = CloudShadowUniforms {
            sun_direction: sky.light().direction.to_array(),
            strength: self.uniforms.bake[1],
            wind_offset: [wind.x, wind.z],
            camera: [view.position.x, view.position.z],
            size: self.tier.ground_size as f32,
            enabled: if self.active { 1.0 } else { 0.0 },
            padding: [0.0; 2],
            clearing: self
                .clearing
                .map_or([0.0; 4], |c| [c.center.x, c.center.y, c.reach, c.fade]),
        };
        queue.write_buffer(
            &self.shadow_interface.uniforms,
            0,
            bytemuck::bytes_of(&uniforms),
        );
    }

    /// Timestamp writes for a pass: the renderer core's timer when given (as `label`), else
    /// [`CloudLayer::set_timestamps`]' query set at `first` and `first + 1`.
    fn timestamp_writes<'a>(
        &'a self,
        timer: Option<&'a naval_render::GpuTimer>,
        label: &str,
        first: u32,
    ) -> Option<wgpu::ComputePassTimestampWrites<'a>> {
        if let Some(timer) = timer {
            return timer.compute_pass(label);
        }
        self.timestamps
            .as_ref()
            .map(|query_set| wgpu::ComputePassTimestampWrites {
                query_set,
                beginning_of_pass_write_index: Some(first),
                end_of_pass_write_index: Some(first + 1),
            })
    }

    /// The shadow maps, their temporal pass and a slice of the ground shadow, when an update is due. Before the scene
    /// pass: they read no depth.
    pub fn encode_shadows(&self, encoder: &mut wgpu::CommandEncoder) {
        self.encode_shadows_timed(encoder, None);
    }

    /// [`CloudLayer::encode_shadows`], timed by the renderer core's timer as `cloud shadow maps`, `cloud shadow
    /// resolve` and `cloud ground shadow`.
    pub fn encode_shadows_timed(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        timer: Option<&naval_render::GpuTimer>,
    ) {
        let plan = self.plan;
        if !plan.update {
            return;
        }
        let size = self.tier.shadow.map_size;
        let cascades = self.tier.shadow.cascade_count as u32;
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cloud shadow maps"),
                timestamp_writes: self.timestamp_writes(timer, "cloud shadow maps", 0),
            });
            pass.set_pipeline(&self.shadow_pipeline);
            pass.set_bind_group(2, &self.shadow_field_group, &[]);
            pass.set_bind_group(3, &self.shadow_out_group, &[]);
            // The marched cascades only (`clouds.cascades.x`, a bit each); the temporal pass carries the others over.
            pass.dispatch_workgroups(
                workgroups(size, 8),
                workgroups(size, 8),
                plan.shadow_count.clamp(1, cascades),
            );
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cloud shadow resolve"),
                timestamp_writes: self.timestamp_writes(timer, "cloud shadow resolve", 2),
            });
            pass.set_pipeline(&self.shadow_resolve_pipeline);
            pass.set_bind_group(2, &self.uniforms_group, &[]);
            pass.set_bind_group(3, &self.shadow_resolve_groups[plan.shadow_from], &[]);
            pass.dispatch_workgroups(workgroups(size, 8), workgroups(size, 8), cascades);
        }
        if let Some((_, count)) = plan.ground {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cloud ground shadow"),
                timestamp_writes: self.timestamp_writes(timer, "cloud ground shadow", 4),
            });
            pass.set_pipeline(&self.ground_pipeline);
            pass.set_bind_group(2, &self.shadow_field_group, &[]);
            pass.set_bind_group(3, &self.ground_out_group, &[]);
            // A whole 1024² map is 16,384 groups of 64, under the per-dimension limit.
            pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
        }
    }

    /// The march and the reconstruction over the opaque scene's `depth` (`scene.opaque.depth`, reversed; `None` for no
    /// scene), `depth_key` naming that texture (its registry version) so the bind groups are made once per texture.
    pub fn encode_march(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        sky: &Sky,
        depth: Option<(&wgpu::TextureView, u64)>,
    ) {
        self.encode_march_timed(device, encoder, sky, depth, None);
    }

    /// [`CloudLayer::encode_march`], timed by the renderer core's timer as `cloud march` and `cloud resolve`.
    pub fn encode_march_timed(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        sky: &Sky,
        depth: Option<(&wgpu::TextureView, u64)>,
        timer: Option<&naval_render::GpuTimer>,
    ) {
        // Key 0 is the layer's own empty depth.
        let (depth_view, key) = match depth {
            Some((view, version)) => (view.clone(), version.wrapping_add(1)),
            None => (self.empty_depth.view.clone(), 0),
        };
        // The composite tests the clouds against the same depth, even on frames that march nothing.
        if self.composite_depth.0 != key {
            self.composite_depth = (
                key,
                gpu::bind_group(
                    device,
                    "cloud composite depth",
                    &self.composite_depth_layout,
                    &[
                        (0, wgpu::BindingResource::TextureView(&depth_view)),
                        (
                            1,
                            wgpu::BindingResource::TextureView(&self.underwater_probe),
                        ),
                    ],
                ),
            );
        }
        let plan = self.plan;
        if !plan.update {
            return;
        }
        if self
            .march_groups
            .as_ref()
            .is_none_or(|groups| groups.depth_key != key)
        {
            let make = |history: usize| {
                let mut entries = field_resources(
                    &self.buffer,
                    &self.weather,
                    &self.shape,
                    &self.detail,
                    &self.turbulence,
                    &self.blue_noise,
                    &self.samplers,
                );
                entries.push((
                    6,
                    wgpu::BindingResource::TextureView(&self.shadow_targets.history[history].view),
                ));
                entries.push((11, wgpu::BindingResource::TextureView(&depth_view)));
                entries.push((
                    12,
                    wgpu::BindingResource::TextureView(&self.underwater_probe),
                ));
                gpu::bind_group(device, "cloud march field", &self.march_layout, &entries)
            };
            self.march_groups = Some(MarchGroups {
                depth_key: key,
                groups: [make(0), make(1)],
            });
        }
        let groups = self.march_groups.as_ref().expect("made above");
        // The march reads the shadow maps' history the temporal pass just wrote.
        let shadow_written = 1 - plan.shadow_from;
        if plan.full {
            // After a cut, every pixel straight into the history the composite reads; no reconstruction.
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cloud full march"),
                timestamp_writes: self.timestamp_writes(timer, "cloud march", 6),
            });
            pass.set_pipeline(&self.march_full_pipeline);
            pass.set_bind_group(1, sky.bind_group(), &[]);
            pass.set_bind_group(2, &groups.groups[shadow_written], &[]);
            pass.set_bind_group(3, &self.march_full_out_groups[1 - plan.clouds_from], &[]);
            pass.dispatch_workgroups(
                workgroups(plan.screen_size[0], 8),
                workgroups(plan.screen_size[1], 8),
                1,
            );
            return;
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cloud march"),
                timestamp_writes: self.timestamp_writes(timer, "cloud march", 6),
            });
            pass.set_pipeline(&self.march_pipeline);
            pass.set_bind_group(1, sky.bind_group(), &[]);
            pass.set_bind_group(2, &groups.groups[shadow_written], &[]);
            pass.set_bind_group(3, &self.march_out_group, &[]);
            pass.dispatch_workgroups(
                workgroups(plan.march_size[0], 8),
                workgroups(plan.march_size[1], 8),
                1,
            );
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cloud resolve"),
                timestamp_writes: self.timestamp_writes(timer, "cloud resolve", 8),
            });
            pass.set_pipeline(&self.resolve_pipeline);
            // The opaque depth, so the reconstruction keeps the scene's silhouette out of the clouds.
            pass.set_bind_group(1, &self.composite_depth.1, &[]);
            pass.set_bind_group(2, &self.uniforms_group, &[]);
            pass.set_bind_group(3, &self.resolve_groups[plan.clouds_from], &[]);
            pass.dispatch_workgroups(
                workgroups(plan.screen_size[0], 8),
                workgroups(plan.screen_size[1], 8),
                1,
            );
        }
    }

    /// The ocean's underwater probe (`naval_ocean::Ocean::underwater_probe`: each pixel's near-plane point's depth below
    /// the waves, positive under, never positive while the near plane cannot reach the water), which the march and the
    /// composite read so a ray that starts under the waves draws no clouds, haze or shaft. `None`: no ray does.
    pub fn set_underwater_probe(&mut self, probe: Option<wgpu::TextureView>) {
        self.underwater_probe = probe.unwrap_or_else(|| self.no_probe.view.clone());
        // The groups that hold it are made again on the next march (no depth key reaches `u64::MAX`).
        self.march_groups = None;
        self.composite_depth.0 = u64::MAX;
        // The screen's history holds the old probe's masking: the next update marches every pixel, and a held or paused
        // picture that had converged converges again. The shadow maps do not read the probe.
        self.has_history = false;
        self.held_updates = 0;
        self.still_updates = 0;
    }

    /// Whether the composite has anything to draw.
    pub fn drawn(&self) -> bool {
        self.active && self.has_history
    }

    /// A handle on what the composite draws each frame, for the layers drawn after it that the clouds should hide
    /// (`naval_sky::CloudCover`; the combat effects' smoke).
    pub fn cover(&self) -> naval_sky::CloudCover {
        self.cover.clone()
    }

    /// Publishes what the composite draws in the renderer core's `frame` (after [`CloudLayer::prepare`]), or that it
    /// draws nothing (`drawn` false: the sky switched off).
    pub fn publish_cover(&self, frame: u64, drawn: bool) {
        let made = self.screens_made;
        let current = (drawn && self.drawn()).then_some(self.current);
        let heights = self.uniforms.heights;
        let histories = &self.screen.history_color;
        self.cover.publish(|cover| {
            if cover.histories.is_none() || cover.version != made {
                cover.histories = Some([histories[0].view.clone(), histories[1].view.clone()]);
                cover.version = made;
            }
            cover.frame = Some(frame);
            cover.current = current;
            cover.bottom = heights[0];
            cover.top = heights[1];
            cover.planet_radius = PLANET_RADIUS;
        });
    }

    /// Draws the composite into the scene pass, after the dome (the `Sky` phase): the latest reconstruction over
    /// everything drawn, untested against depth.
    pub fn draw_composite(&self, pass: &mut wgpu::RenderPass<'_>) {
        if !self.drawn() {
            return;
        }
        pass.set_pipeline(&self.composite_pipeline);
        pass.set_bind_group(1, &self.composite_groups[self.current], &[]);
        pass.set_bind_group(2, &self.uniforms_group, &[]);
        pass.set_bind_group(3, &self.composite_depth.1, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// The field's resources at group 2: the uniforms, the five textures and the two samplers.
fn field_resources<'a>(
    buffer: &'a wgpu::Buffer,
    weather: &'a Texture,
    shape: &'a Texture,
    detail: &'a Texture,
    turbulence: &'a Texture,
    blue_noise: &'a Texture,
    samplers: &'a [wgpu::Sampler; 2],
) -> Vec<(u32, wgpu::BindingResource<'a>)> {
    use wgpu::BindingResource as R;
    vec![
        (0, buffer.as_entire_binding()),
        (1, R::TextureView(&weather.view)),
        (2, R::TextureView(&shape.view)),
        (3, R::TextureView(&detail.view)),
        (4, R::TextureView(&turbulence.view)),
        (5, R::TextureView(&blue_noise.view)),
        (7, R::Sampler(&samplers[0])),
        (8, R::Sampler(&samplers[1])),
    ]
}

type FieldTextures<'a> = (
    &'a wgpu::Buffer,
    &'a Texture,
    &'a Texture,
    &'a Texture,
    &'a Texture,
    &'a Texture,
    &'a [wgpu::Sampler; 2],
);

/// The groups that hold the screen's targets: the hooks' field (with both shaft histories), the march's outputs, the
/// full march's outputs (history `i`), the reconstruction (reading history `i`, writing the other) and the composite
/// (reading history `i`).
fn screen_groups(
    device: &wgpu::Device,
    field_layout: &wgpu::BindGroupLayout,
    march_out_layout: &wgpu::BindGroupLayout,
    resolve_layout: &wgpu::BindGroupLayout,
    composite_layout: &wgpu::BindGroupLayout,
    screen: &ScreenTargets,
    (buffer, weather, shape, detail, turbulence, blue_noise, samplers): FieldTextures<'_>,
) -> (
    wgpu::BindGroup,
    wgpu::BindGroup,
    [wgpu::BindGroup; 2],
    [wgpu::BindGroup; 2],
    [wgpu::BindGroup; 2],
) {
    use wgpu::BindingResource as R;
    let mut entries = field_resources(
        buffer, weather, shape, detail, turbulence, blue_noise, samplers,
    );
    entries.push((9, R::TextureView(&screen.history_shadow_length[0].view)));
    entries.push((10, R::TextureView(&screen.history_shadow_length[1].view)));
    let field = gpu::bind_group(device, "cloud field and shafts", field_layout, &entries);
    let march_out = gpu::bind_group(
        device,
        "cloud march out",
        march_out_layout,
        &[
            (0, R::TextureView(&screen.march_color.view)),
            (1, R::TextureView(&screen.march_depth_velocity.view)),
            (2, R::TextureView(&screen.march_shadow_length.view)),
        ],
    );
    // The full march writes the drawing buffer's history directly; the motion target (binding 1) is the march
    // buffer's, which it leaves alone.
    let march_full = |into: usize| {
        gpu::bind_group(
            device,
            "cloud full march out",
            march_out_layout,
            &[
                (0, R::TextureView(&screen.history_color[into].view)),
                (1, R::TextureView(&screen.march_depth_velocity.view)),
                (2, R::TextureView(&screen.history_shadow_length[into].view)),
            ],
        )
    };
    let resolve = |from: usize| {
        gpu::bind_group(
            device,
            "cloud resolve",
            resolve_layout,
            &[
                (0, R::TextureView(&screen.march_color.view)),
                (1, R::TextureView(&screen.march_depth_velocity.view)),
                (2, R::TextureView(&screen.march_shadow_length.view)),
                (3, R::TextureView(&screen.history_color[from].view)),
                (4, R::TextureView(&screen.history_shadow_length[from].view)),
                (5, R::TextureView(&screen.history_color[1 - from].view)),
                (
                    6,
                    R::TextureView(&screen.history_shadow_length[1 - from].view),
                ),
            ],
        )
    };
    // The composite reads the history's colour and, for its spatial filter, how many samples each pixel holds.
    let composite = |current: usize| {
        gpu::bind_group(
            device,
            "cloud composite",
            composite_layout,
            &[
                (0, R::TextureView(&screen.history_color[current].view)),
                (
                    1,
                    R::TextureView(&screen.history_shadow_length[current].view),
                ),
            ],
        )
    };
    (
        field,
        march_out,
        [march_full(0), march_full(1)],
        [resolve(0), resolve(1)],
        [composite(0), composite(1)],
    )
}

/// The shadow maps' temporal pass reading history `i` and writing the other.
fn shadow_resolve_groups(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    targets: &ShadowTargets,
) -> [wgpu::BindGroup; 2] {
    use wgpu::BindingResource as R;
    let group = |from: usize| {
        gpu::bind_group(
            device,
            "cloud shadow maps resolve",
            layout,
            &[
                (0, R::TextureView(&targets.current.view)),
                (1, R::TextureView(&targets.depth_velocity.view)),
                (2, R::TextureView(&targets.history[from].view)),
                (3, R::TextureView(&targets.history[1 - from].view)),
            ],
        )
    };
    [group(0), group(1)]
}

/// The composite: dst = src + dst × (1 − src.a) over the scene, its depth untested and unwritten, the scene's alpha
/// left alone; group 0 (the renderer's camera) left empty, `layouts` the composite's own (1), the uniforms (2) and the
/// opaque scene's depth (3).
fn composite_pipeline(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layouts: &[&wgpu::BindGroupLayout; 3],
    targets: Targets,
    cache: Option<&wgpu::PipelineCache>,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("cloud composite"),
        bind_group_layouts: &[None, Some(layouts[0]), Some(layouts[1]), Some(layouts[2])],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("cloud composite"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("composite_vertex"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: targets.depth.map(|format| wgpu::DepthStencilState {
            format,
            depth_write_enabled: Some(false),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("composite_fragment"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: targets.color,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::Zero,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Add,
                    },
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache,
    })
}

fn make_shadow_interface(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    map: &Texture,
    sampler: &wgpu::Sampler,
    uniforms: wgpu::Buffer,
) -> CloudShadow {
    use wgpu::BindingResource as R;
    let bind_group = gpu::bind_group(
        device,
        "cloud shadow",
        layout,
        &[
            (0, R::TextureView(&map.view)),
            (1, R::Sampler(sampler)),
            (2, uniforms.as_entire_binding()),
        ],
    );
    CloudShadow {
        view: map.view.clone(),
        sampler: sampler.clone(),
        uniforms,
        bind_group_layout: layout.clone(),
        bind_group,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blue_cloud_experiment_fitting_preserves_scene_projection_and_depth() {
        let near = 0.1;
        let far = 259.8076;
        let view = CloudView { position: Vec3::new(0.0, 25.0, 0.0),
            view: glam::camera::rh::view::look_to_mat4(Vec3::new(0.0, 25.0, 0.0), Vec3::NEG_Z, Vec3::Y),
            projection: glam::camera::rh::proj::directx::perspective(1.1, 16.0 / 9.0, far, near), near, far,
            viewport: [1280, 720] };
        assert_eq!(CloudTuning::default().shadow_view(&view).projection, view.projection);
        for shadow_far in [10_000.0, 60_000.0, 200_000.0] {
            let tuning = CloudTuning { shadow_far, ..Default::default() };
            let fitted = tuning.shadow_view(&view);
            let corners = crate::cascades::FrustumCorners::from_projection(fitted.projection, fitted.far);
            for corner in corners.far {
                assert!((-corner.z - shadow_far).abs() < shadow_far * 1e-5);
            }
            assert_eq!(fitted.view, view.view);
            assert_eq!(fitted.position, view.position);
            for distance in [0.1, 1.0, 25.0, 200.0] {
                let clip = view.projection * Vec3::new(0.0, 0.0, -distance).extend(1.0);
                let depth = clip.z / clip.w;
                let decoded = far * near / (depth * (far - near) + near);
                assert!((decoded - distance).abs() < distance * 1e-5);
            }
        }
        let bad = CloudTuning { shadow_far: f32::NAN, far_step_scale: f32::INFINITY,
            debug: 255, shadow_mode: 255, sun_steps: 99 }.sanitize();
        assert_eq!(bad.shadow_far, 0.0);
        assert_eq!(bad.far_step_scale, 1.0);
        assert_eq!((bad.debug, bad.shadow_mode, bad.sun_steps), (5, 1, 8));
    }

    /// Read actual output pixels: reduced histories must cover the whole viewport,
    /// preserve their orientation and stay behind full-resolution foreground depth.
    #[test]
    fn scaled_cloud_composite_preserves_viewport_and_foreground_occlusion() {
        futures_lite::future::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance.request_adapter(&Default::default()).await.expect("GPU adapter");
            let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(), ..Default::default()
            }).await.expect("GPU device");
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let sky = Sky::new(&device, &queue, SkyQuality::Low, Default::default());
            let mut layer = CloudLayer::new(&device, &queue, &sky, SkyQuality::Low,
                Targets { color: wgpu::TextureFormat::Rgba16Float, depth: None });
            let viewport = [65, 33];
            let make_target = |format, usage| device.create_texture(&wgpu::TextureDescriptor {
                label: Some("scaled cloud composite fixture"),
                size: wgpu::Extent3d { width: viewport[0], height: viewport[1], depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                format, usage, view_formats: &[],
            });
            let output = make_target(wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC);
            let output_view = output.create_view(&Default::default());
            let depth = make_target(wgpu::TextureFormat::Depth32Float,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
            let depth_view = depth.create_view(&Default::default());
            layer.composite_depth.1 = gpu::bind_group(&device, "fixture depth", &layer.composite_depth_layout, &[
                (0, wgpu::BindingResource::TextureView(&depth_view)),
                (1, wgpu::BindingResource::TextureView(&layer.underwater_probe)),
            ]);
            let view = CloudView {
                position: Vec3::ZERO, view: Mat4::IDENTITY,
                projection: glam::camera::rh::proj::directx::perspective(1.0, 65.0 / 33.0, 6000.0, 0.1),
                near: 0.1, far: 6000.0, viewport,
            };
            let stride = 768;
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("cloud pixel readback"), size: u64::from(stride * viewport[1]),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false,
            });
            for scale in [1.0, 0.75, 0.5, 0.25, 1.0] {
                layer.set_render_scale(scale);
                layer.prepare(&device, &mut naval_render::UploadSink::Queue(&queue), &view, &sky, false);
                // Four solid quadrants, already converged. Exact half-float values
                // make an incorrect screen/depth coordinate mapping unambiguous.
                let mut colors = Vec::<u16>::new();
                let mut states = Vec::<u16>::new();
                for y in 0..layer.height {
                    for x in 0..layer.width {
                        colors.extend_from_slice(&[if x < layer.width / 2 { 0x3400 } else { 0x3a00 },
                            if y < layer.height / 2 { 0x3400 } else { 0x3a00 }, 0, 0x3c00]);
                        states.extend_from_slice(&[0, 0x4800, 0, 0]);
                    }
                }
                for (texture, pixels) in [(&layer.screen.history_color[layer.current].texture, colors),
                    (&layer.screen.history_shadow_length[layer.current].texture, states)] {
                    queue.write_texture(texture.as_image_copy(), bytemuck::cast_slice(&pixels),
                        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(layer.width * 8), rows_per_image: None },
                        wgpu::Extent3d { width: layer.width, height: layer.height, depth_or_array_layers: 1 });
                }
                for blocked in [false, true] {
                    let mut encoder = device.create_command_encoder(&Default::default());
                    {
                        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                                view: &depth_view, depth_ops: Some(wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(if blocked { 1.0 } else { 0.0 }),
                                    store: wgpu::StoreOp::Store }), stencil_ops: None }),
                            ..Default::default()
                        });
                    }
                    {
                        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &output_view, depth_slice: None, resolve_target: None,
                                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store }
                            })], ..Default::default()
                        });
                        layer.draw_composite(&mut pass);
                    }
                    encoder.copy_texture_to_buffer(output.as_image_copy(), wgpu::TexelCopyBufferInfo {
                        buffer: &readback, layout: wgpu::TexelCopyBufferLayout { offset: 0,
                            bytes_per_row: Some(stride), rows_per_image: None }
                    }, output.size());
                    queue.submit([encoder.finish()]);
                    let (tx, rx) = std::sync::mpsc::channel();
                    readback.slice(..).map_async(wgpu::MapMode::Read, move |result| { tx.send(result).unwrap(); });
                    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
                    rx.recv().unwrap().unwrap();
                    let pixels = readback.slice(..).get_mapped_range().unwrap();
                    for (x, y) in [(4, 4), (60, 4), (4, 28), (60, 28)] {
                        let at = (y * stride + x * 8) as usize;
                        let pixel: Vec<_> = pixels[at..at + 8].chunks_exact(2)
                            .map(|v| u16::from_le_bytes([v[0], v[1]])).collect();
                        let expected = if blocked { vec![0; 4] } else {
                            // The upstream composite preserves destination alpha.
                            vec![if x < 32 { 0x3400 } else { 0x3a00 }, if y < 16 { 0x3400 } else { 0x3a00 }, 0, 0]
                        };
                        assert_eq!(pixel, expected, "scale {scale}, blocked {blocked}, pixel ({x},{y})");
                    }
                    drop(pixels);
                    readback.unmap();
                }
            }
            assert!(scope.pop().await.is_none(), "scaled cloud GPU validation");
        });
    }

    /// The WGSL struct's size and a few offsets, as naga lays them out, equal the Rust struct's.
    #[test]
    fn rust_and_wgsl_layouts_agree() {
        let module =
            crate::shaders::compose(crate::shaders::RESOLVE_WGSL, "resolve.wgsl", &[]).unwrap();
        let (members, span) = module
            .types
            .iter()
            .find_map(|(_, ty)| match (&ty.name, &ty.inner) {
                (Some(name), wgpu::naga::TypeInner::Struct { members, span })
                    if name.starts_with("CloudUniforms") =>
                {
                    Some((members.clone(), *span))
                }
                _ => None,
            })
            .expect("CloudUniforms in the resolve's module");
        let offset_of = |field: &str| {
            members
                .iter()
                .find(|m| m.name.as_deref() == Some(field))
                .map(|m| m.offset)
        };
        assert_eq!(
            offset_of("camera"),
            Some(std::mem::offset_of!(CloudUniforms, camera) as u32)
        );
        assert_eq!(
            offset_of("channels"),
            Some(std::mem::offset_of!(CloudUniforms, channels) as u32)
        );
        assert_eq!(
            offset_of("shadow_intervals"),
            Some(std::mem::offset_of!(CloudUniforms, shadow_intervals) as u32)
        );
        assert_eq!(
            offset_of("shadow_reprojection"),
            Some(std::mem::offset_of!(CloudUniforms, shadow_reprojection) as u32)
        );
        assert_eq!(
            offset_of("bake"),
            Some(std::mem::offset_of!(CloudUniforms, bake) as u32)
        );
        assert_eq!(span as usize, std::mem::size_of::<CloudUniforms>());
    }

    #[test]
    fn the_bayer_cells_cover_the_block_in_sixteen_updates() {
        let mut seen = [[false; 4]; 4];
        for frame in 0..16 {
            let [x, y] = bayer_cell(frame);
            assert!(!seen[y as usize][x as usize], "cell {x},{y} twice");
            seen[y as usize][x as usize] = true;
        }
        assert_eq!(bayer_cell(0), [0, 0]);
        assert_eq!(bayer_cell(16), bayer_cell(0));
    }
}

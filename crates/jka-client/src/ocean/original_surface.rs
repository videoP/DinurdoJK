//! JKA host inputs for Blue Water's original ocean material and clipmap.
use super::{OCEAN_MAX_SURFACES, OCEAN_UNITS_PER_METER, OceanGpu, OceanPvsTileMask};
use blue_water_gpu::{
    foam_texture::FoamMap,
    original_surface::{
        OriginalClipmap, OriginalSurfaceBindings, OriginalSurfacePipeline, SurfaceAppearance,
        SurfaceBoundary, SurfaceCamera,
    },
    scene_copy::SceneCopy,
    underwater::{UnderwaterBindings, UnderwaterPipelines},
};
use bytemuck::Zeroable;
use glam::{Mat4, Vec3};

/// Render original Blue Water optics at 1/2 or 1/4 framebuffer resolution.
/// The full-resolution reconstruction still rasterizes the actual displaced
/// clipmap against the host's depth buffer and JKA footprint mask.
struct ReducedOcean {
    _color: wgpu::Texture,
    color_view: wgpu::TextureView,
    _depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    binding: wgpu::BindGroup,
}

impl ReducedOcean {
    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, width: u32, height: u32, factor: u32) -> Self {
        let extent = wgpu::Extent3d {
            width: width.max(1).div_ceil(factor),
            height: height.max(1).div_ceil(factor),
            depth_or_array_layers: 1,
        };
        let _color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Blue Water shaded at reduced resolution"),
            size: extent, mip_level_count: 1, sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let color_view = _color.create_view(&Default::default());
        let _depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Blue Water reduced depth for edge-aware upscaling"),
            size: extent, mip_level_count: 1, sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_view = _depth.create_view(&Default::default());
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blue Water reduced shaded color/depth"), layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&color_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&depth_view) },
            ],
        });
        Self { _color, color_view, _depth, depth_view, binding }
    }
}

/// Foam needs both visible wave energy and something capable of breaking it.
/// A low significant height at 0.1 m/s is not a 50%-foamy version of a storm.
/// Treat energetic local wind OR steep incoming swell as a source of breakers,
/// while leaving a tiny, continuous response for any positive wave height.
/// This affects only *wave-generated* foam, not shores or contact/wakes.
fn native_foam_energy(waves: super::BlueWaterWaves) -> f32 {
    let height = waves.height.max(0.0);
    if height == 0.0 { return 0.0; }
    let smooth = |start: f32, end: f32, value: f32| {
        let t = ((value - start) / (end - start)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    // Significant wave height is in metres. Squaring removes the substantial
    // low-sea foam left by the previous linear response, without a hard cutoff.
    let size = (height / (height + 1.8)).powi(2);
    let wind = smooth(1.0, 12.0, waves.wind_speed.max(0.0));
    // Existing swell can still break after local wind dies down. A long, flat
    // swell cannot; at the reported 0.5 m / 512 m this term is effectively 0.
    let steepness = height * waves.choppiness.max(0.0) / waves.wavelength.max(2.0);
    let swell = smooth(0.025, 0.10, steepness);
    size * (0.015 + 0.985 * wind.max(swell))
}

/// Retain original coverage at the shipped 1.8 m / 9 m/s sea state. This
/// scalar is passed to the existing shader foam opacity controls once per
/// visible promoted surface; no extra per-fragment branching or texture work.
fn native_foam_strength(waves: super::BlueWaterWaves) -> f32 {
    static DEFAULT: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    let reference = *DEFAULT.get_or_init(|| native_foam_energy(super::BlueWaterWaves::default()).max(1.0e-6));
    (native_foam_energy(waves) / reference).clamp(0.0, 1.0)
}

/// Blue Water's original shader evaluates stretch = 1 + 2 * wind_stretch.
/// Calm wind -> rounder 1:1 patterns, strong wind -> original 2:1 patterns.
/// Matches the *spectrum* wind rather than a second unrelated weather clock.
fn native_foam_stretch(wind_metres_per_second: f32) -> f32 {
    let t = ((wind_metres_per_second.max(0.0) - 2.0) / 18.0).clamp(0.0, 1.0);
    0.5 * t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod automatic_foam_tests {
    use super::{native_foam_strength, native_foam_stretch};

    #[test]
    fn flat_is_zero_and_tiny_amplitude_is_positive() {
        let baseline = super::super::BlueWaterWaves::default();
        assert_eq!(native_foam_strength(super::super::BlueWaterWaves { height: 0.0, ..baseline }), 0.0);
        assert!(native_foam_strength(super::super::BlueWaterWaves { height: 0.000025, ..baseline }) > 0.0);
        assert_eq!(native_foam_strength(baseline), 1.0);
    }

    #[test]
    fn light_wind_and_flat_swell_do_not_paint_the_sea_white() {
        let calm = super::super::BlueWaterWaves { height: 0.5, wind_speed: 0.1,
            wavelength: 512.0, choppiness: 1.45, ..super::super::BlueWaterWaves::default() };
        // The screenshot's settings must leave under 2% of normal wave foam.
        assert!(native_foam_strength(calm) < 0.02);
        assert!(native_foam_strength(calm) > 0.0);
        assert!(native_foam_strength(super::super::BlueWaterWaves { wind_speed: 10.0, ..calm })
            > native_foam_strength(calm));
        // Even without local wind, a genuinely steep swell can still break.
        assert!(native_foam_strength(super::super::BlueWaterWaves { height: 2.0,
            wavelength: 10.0, choppiness: 2.0, ..calm }) > native_foam_strength(calm));
    }

    #[test]
    fn wind_stretch_remains_round_at_calm_and_elongated_at_gale() {
        assert_eq!(native_foam_stretch(0.0), 0.0);
        assert_eq!(native_foam_stretch(2.0), 0.0);
        assert!(native_foam_stretch(10.0) > 0.0);
        assert!(native_foam_stretch(10.0) < 0.5);
        assert_eq!(native_foam_stretch(20.0), 0.5);
    }
}

fn pixel_factor(mode: u8) -> u32 {
    match mode { 1 | 3 => 2, 2 | 4 => 4, _ => 1 }
}

/// Per-ring LOD for Blue Water, retaining the upstream full-density clipmap
/// through the three rings nearest the water's *active finest* ring. A distant
/// triangle's narrow projection is controlled by elevation/range, NOT by the
/// camera pitch alone. Reducing all `segments` made nearby waves lose detail
/// while leaving an expensive fully tessellated horizon.
///
/// Maximum simplification ramps outward: 1,1,1,2,4,8,16. The boundary of
/// EVERY ring remains on the upstream original lattice; only its interior is
/// decimated by OriginalClipmap's prebaked, stitched index variants.
fn projected_ring_steps(
    clipmap: blue_water_gpu::geometry::Clipmap,
    first: usize,
    camera_height_metres: f32,
    viewport_height: u32,
    tan_half_fovy: f32,
    target_pixels: f32,
) -> [u8; 10] {
    let mut steps = [1; 10];
    if target_pixels <= 0.0 { return steps; }
    let eye_height = camera_height_metres.abs().max(0.001);
    let focal_length = viewport_height.max(1) as f32 / (2.0 * tan_half_fovy.max(0.001));
    for (level, step) in steps.iter_mut().take(clipmap.levels).enumerate() {
        // Never simplify the nearest three DRAWN rings (not merely rings 0-2).
        // Elevated cameras can start at a coarser first ring in upstream detail().
        let max_shift = level.saturating_sub(first.saturating_add(2)).min(4);
        if max_shift == 0 { continue; }
        let range = (clipmap.half(level) * 0.75) as f32;
        let distance = (range * range + eye_height * eye_height).sqrt();
        // Projected thickness perpendicular to the grazing water surface.
        // Goes to zero at the horizon even if pixels cover a huge horizontal
        // span; only then should outer-ring triangles become very coarse.
        let projected = focal_length * clipmap.cell(level) as f32
            * (eye_height / distance) / distance;
        let maximum = 1u8 << max_shift;
        while *step < maximum && projected * (*step as f32) < target_pixels {
            *step *= 2;
        }
    }
    steps
}

/// Conservative screen-space error bound for a sinusoidal displaced surface.
/// For a smooth wave z=A sin(kx), chord error <= A*k²*width²/8. This is an
/// estimate (not a readback of the live FFT), intentionally biased toward
/// retaining detail on steep/short/choppy waves. Zero amplitude selects the
/// maximum safe stride, regardless of distance.
fn curvature_mesh_steps(
    clipmap: blue_water_gpu::geometry::Clipmap,
    first: usize,
    eye_height: f32,
    viewport_height: u32,
    tan_half_fovy: f32,
    wave_height: f32,
    wavelength: f32,
    choppiness: f32,
    wind_speed: f32,
    analytic: [f32; 4], // direction.xy, k, amplitude (metres)
) -> ([u8; 10], u8) {
    let mut steps = [1u8; 10];
    let focal = viewport_height.max(1) as f32 / (2.0 * tan_half_fovy.max(0.001));
    let peak = wavelength.max(2.0);
    let h = wave_height.max(0.0);
    // Short waves become relevant as the wind builds; do not infer a high
    // curvature wind-chop tail at 0.1 m/s from the spectrum peak alone.
    let wind_factor = (wind_speed.max(0.0) / 12.0).clamp(0.0, 2.0);
    let tail = (wind_factor * choppiness.max(0.0)).sqrt();
    let effective_wavelength = peak / (1.0 + 1.5 * tail);
    let k = std::f32::consts::TAU / effective_wavelength;
    let fft_curvature = 0.5 * h * k * k * (1.0 + 0.3 * tail);
    let analytic_curvature = analytic[3].abs() * analytic[2] * analytic[2];
    let curvature = fft_curvature + analytic_curvature;
    if !curvature.is_finite() { return (steps, 1); }
    let test = |cell: f32, level: usize, middle: bool| -> u8 {
        let horizontal = if middle { cell * 2.0 }
            else { (clipmap.half(level) * 0.6) as f32 };
        let dist = eye_height.abs().hypot(horizontal).max(cell);
        let mut step = 1;
        while step < 16 {
            let proposed = step * 2;
            let width = cell * proposed as f32;
            let sagitta_pixels = curvature * width * width * focal / (8.0 * dist);
            let normal_change = curvature * width;
            if sagitta_pixels > 1.5 || normal_change > 0.12 { break; }
            step = proposed;
        }
        step
    };
    for level in first..clipmap.levels {
        steps[level] = test(clipmap.cell(level) as f32, level, false);
    }
    let fill_step = test(clipmap.cell(first) as f32, first, true);
    (steps, fill_step)
}

#[cfg(test)]
mod projected_lod_tests {
    use super::projected_ring_steps;
    use blue_water_gpu::geometry::Clipmap;

    #[test]
    fn curvature_lod_flat_water_reaches_coarsest_fill_and_ring_stride() {
        let clip = Clipmap::new(64, 6);
        let (steps, fill) = super::curvature_mesh_steps(
            clip, 0, 1.0, 900, 0.5, 0.0, 512.0, 0.0, 0.1, [1.0, 0.0, 0.01, 0.0]);
        assert_eq!(fill, 16);
        assert!(steps[..6].iter().all(|&step| step == 16));
    }

    #[test]
    fn curvature_lod_keeps_short_steep_waves_fine() {
        let clip = Clipmap::new(64, 6);
        let (flat, _) = super::curvature_mesh_steps(
            clip, 0, 1.0, 900, 0.5, 0.0, 512.0, 0.0, 0.1, [1.0, 0.0, 0.01, 0.0]);
        let (rough, fill) = super::curvature_mesh_steps(
            clip, 0, 1.0, 900, 0.5, 6.0, 12.0, 2.0, 15.0, [1.0, 0.0, 0.0, 0.0]);
        assert!(rough[0] < flat[0]);
        assert_eq!(fill, 1);
    }

    #[test]
    fn near_rings_are_always_original_quality() {
        let clip = Clipmap::new(128, 9);
        for first in [0, 2, 5] {
            for height in [0.1, 1.0, 20.0, 1000.0] {
                let steps = projected_ring_steps(clip, first, height, 900, 0.5, 8.0);
                for level in first..(first + 3).min(clip.levels) {
                    assert_eq!(steps[level], 1, "first={first}, level={level}");
                }
            }
        }
    }

    #[test]
    fn shallow_angle_collapses_outer_rings_but_not_inner() {
        let clip = Clipmap::new(128, 9);
        let steps = projected_ring_steps(clip, 0, 1.0, 900, 0.5, 4.0);
        assert_eq!(&steps[..3], &[1, 1, 1]);
        assert!(steps[7] > steps[4]);
        assert!(steps[8] >= 8, "outer steps: {steps:?}");
        assert_eq!(projected_ring_steps(clip, 0, 1.0, 900, 0.5, 0.0), [1; 10]);
    }
}

/// Changes only when the snapped native lattice, ring LOD or Godot's live
/// visible-water tile mask changes. The CPU/GPU mesh never tracks waves.
#[derive(Clone, PartialEq, Eq)]
struct NativePruneKey {
    centres: [[u32; 2]; 10],
    first: usize,
    outer: usize,
    steps: [u8; 10],
    fill_step: u8,
    horizon: bool,
    sky_extension: bool,
    pvs: Option<OceanPvsTileMask>,
}

struct NativePrunedGeometry {
    buffer: wgpu::Buffer,
    count: u32,
    /// Full-density triangles for the SAME wet-area/PVS and sky-extension admission.
    /// Only computed when r_oceanAdaptiveMeshDebug=1.
    baseline_count: Option<u32>,
    upload_generation: u64,
    key: Option<NativePruneKey>,
}

struct SurfaceSlot {
    bindings: OriginalSurfaceBindings,
    first: usize,
    outer: usize,
    ring_steps: [u8; 10],
    fill_step: u8,
    pruned: Option<NativePrunedGeometry>,
    ready: bool,
    horizon: bool,
    underwater: UnderwaterBindings,
    caustic_receiver: blue_water_gpu::caustics::Receiver,
    rain: Option<blue_water_gpu::rain_heights::SurfaceRead>,
    probe_due: bool,
    probe_active: bool,
    envelope: [f32; 4],
    /// The `OriginalSurfaceBindings::wake_generation` the underwater/rain groups were built against.
    wake_generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SurfaceConfig {
    target: (u32, u32, u32, wgpu::TextureFormat),
    reflection_steps: u32,
    pixel_detail_mode: u8,
    segments: u32,
    levels: u8,
}

impl SurfaceConfig {
    fn new(target: (u32, u32, u32, wgpu::TextureFormat), settings: super::OceanSettings) -> Self {
        Self { target, reflection_steps: settings.blue_water_ssr_steps,
            pixel_detail_mode: settings.pixel_detail_mode,
            segments: settings.blue_water_mesh_segments, levels: settings.blue_water_mesh_levels }
    }
}

pub(super) struct SurfaceRenderer {
    config: SurfaceConfig,
    key: (u32, u32, u32, wgpu::TextureFormat),
    pipeline: OriginalSurfacePipeline,
    clipmap: OriginalClipmap,
    reduced: Option<ReducedOcean>,
    scene: SceneCopy,
    map_environment: Option<blue_water_gpu::map_environment::MapEnvironment>,
    sky_faces: Option<[wgpu::TextureView; 6]>,
    external_sky: Option<wgpu::TextureView>,
    foam: FoamMap,
    slots: [Option<SurfaceSlot>; OCEAN_MAX_SURFACES],
    underwater: UnderwaterPipelines,
    caustics: blue_water_gpu::caustics::Caustics,
    rain_heights: Option<blue_water_gpu::rain_heights::RainHeights>,
    underwater_slot: Option<usize>,
    /// Native spindrift (`r_oceanSpindrift`), built on first use at a gale's wind.
    spindrift: Option<blue_water_fx::spindrift::Spindrift>,
    /// This frame's camera x, z (m) and whether the aerial volume is live, for the spindrift draw later in the frame.
    eye: [f64; 2],
    aerial: bool,
}

impl SurfaceRenderer {
    fn build(device: &wgpu::Device, queue: &wgpu::Queue, waves: &wgpu::BindGroupLayout, config: SurfaceConfig,
        sky_faces: Option<[wgpu::TextureView; 6]>) -> Self {
        let key = config.target;
        let pipeline = OriginalSurfacePipeline::new_with_wave_layout(
            device, waves, key.3, key.2, config.reflection_steps, config.pixel_detail_mode);
        let underwater = UnderwaterPipelines::new(device, waves, key.3, key.2);
        let caustics = blue_water_gpu::caustics::Caustics::new(device, waves, key.3, key.2);
        let clipmap = OriginalClipmap::new(device, config.segments, config.levels as usize);
        let scene = SceneCopy::new(device, key.0, key.1, key.2);
        let foam = FoamMap::new(device, queue);
        let reduced = (config.pixel_detail_mode != 0).then(|| ReducedOcean::new(
            device, pipeline.reconstruct_layout.as_ref().expect("reconstruction layout"),
            key.0, key.1, pixel_factor(config.pixel_detail_mode)));
        let map_environment = sky_faces.as_ref().map(|faces| {
            let mut environment = blue_water_gpu::map_environment::MapEnvironment::new(device);
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("native ocean map environment preparation"),
            });
            environment.bake(device, &mut encoder, faces.each_ref());
            queue.submit([encoder.finish()]);
            environment
        });
        Self { config, key, pipeline, underwater, caustics, clipmap, scene, foam, reduced,
            underwater_slot: None, rain_heights: None, spindrift: None, eye: [0.0; 2],
            aerial: false, map_environment, sky_faces, external_sky: None,
            slots: std::array::from_fn(|_| None) }
    }
}

/// Translate the water plane to upstream y=0 and convert map units to metres.
/// This preserves exactly the host's projection, including reversed depth and
/// temporal jitter, while keeping the upstream wave and optical scales intact.
pub(super) fn surface_camera(
    camera: &crate::camera::Camera,
    plane: f32,
    width: u32,
    height: u32,
    jitter: [f32; 2],
    time: f32,
    dt: f32,
    frame: u32,
) -> SurfaceCamera {
    let width = width.max(1) as f32;
    let height = height.max(1) as f32;
    let aspect = width / height;
    let fov = camera.fov_y_for_viewport(width as u32, height as u32);
    let position = (camera.position - Vec3::Y * plane) / OCEAN_UNITS_PER_METER;
    let view = Mat4::look_to_rh(position, camera.forward(), Vec3::Y);
    let near = camera.near_distance() / OCEAN_UNITS_PER_METER;
    let far = camera.far_distance() / OCEAN_UNITS_PER_METER;
    let projection = Mat4::from_translation(Vec3::new(
        2.0 * jitter[0] / width,
        -2.0 * jitter[1] / height,
        0.0,
    )) * Mat4::perspective_rh(fov, aspect, far, near);
    let view_projection = projection * view;
    SurfaceCamera {
        view: view.to_cols_array_2d(),
        projection: projection.to_cols_array_2d(),
        view_projection: view_projection.to_cols_array_2d(),
        inverse_view: view.inverse().to_cols_array_2d(),
        inverse_projection: projection.inverse().to_cols_array_2d(),
        inverse_view_projection: view_projection.inverse().to_cols_array_2d(),
        position_time: [position.x, position.y, position.z, time],
        viewport: [width, height, width.recip(), height.recip()],
        near,
        far,
        tan_half_fov_y: (fov * 0.5).tan(),
        aspect,
        dt,
        frame,
        curvature: 0.0,
        padding: 0,
    }
}

impl OceanGpu {
    /// Prepare the native surface independently of the atmosphere. Until this
    /// returns true, the host must draw the authored water and its optics.
    pub(crate) fn ensure_original_surface(
        &mut self,
        jobs: &mut crate::pipeline_jobs::PipelineJobManager,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: (u32, u32, u32, wgpu::TextureFormat),
        sky_faces: Option<[&wgpu::TextureView; 6]>,
    ) -> bool {
        use crate::pipeline_jobs::{hash_config, PipelineJobKey};
        if !matches!(self.settings.backend, 1 | 2) {
            jobs.cancel_slot("native-ocean-surface", 0);
            return true;
        }
        let config = SurfaceConfig::new(target, self.settings);
        let Some(blue) = self.blue_water.as_mut() else { return false; };
        if blue.original_surface.as_ref().is_some_and(|surface| surface.config == config) {
            return true;
        }
        blue.original_surface = None;
        // The layout handle also identifies the wave resource generation. A
        // late result from a recreated ocean cannot replace the new surface.
        let key = PipelineJobKey::new("native-ocean-surface", 0,
            hash_config(&(config, blue.surface_waves.layout())));
        if let Some(surface) = jobs.take_ready::<SurfaceRenderer>(key) {
            blue.original_surface = Some(surface);
            rverbose!(1, "[ENVIRONMENT READY] ocean surface activated at frame boundary {config:?}");
            return true;
        }
        if jobs.is_compiling(key) { return false; }
        let (device, queue, waves) = (device.clone(), queue.clone(), blue.surface_waves.layout().clone());
        let sky_faces = sky_faces.map(|faces| faces.map(Clone::clone));
        jobs.request(key, "native ocean surface", move || {
            let started = std::time::Instant::now();
            let surface = SurfaceRenderer::build(&device, &queue, &waves, config, sky_faces);
            rverbose!(1, "[ENVIRONMENT PREPARED] ocean surface worker {:.1} ms {config:?}",
                started.elapsed().as_secs_f64() * 1000.0);
            surface
        });
        false
    }

    pub(crate) fn original_surface_ready(&self, target: (u32, u32, u32, wgpu::TextureFormat)) -> bool {
        !matches!(self.settings.backend, 1 | 2) || self.blue_water.as_ref()
            .and_then(|blue| blue.original_surface.as_ref())
            .is_some_and(|surface| surface.config == SurfaceConfig::new(target, self.settings))
    }

    pub(crate) fn native_surface_available(&self) -> bool {
        !matches!(self.settings.backend, 1 | 2) || self.blue_water.as_ref()
            .and_then(|blue| blue.original_surface.as_ref())
            .is_some_and(|surface| surface.config == SurfaceConfig::new(surface.key, self.settings))
    }
    /// Godot's visible_tiles distinguishes unavailable mask data (None, keep
    /// conservative) from known water with zero tiles in this cluster (reject).
    /// Sky continuation is admitted separately by Godot's per-frame gate.
    fn native_pool_visible(pvs: Option<OceanPvsTileMask>, sky_extension: bool) -> bool {
        sky_extension || pvs.is_none_or(|tiles| tiles.iter().any(|word| *word != 0))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare_original_surface(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera: &crate::camera::Camera,
        key: (u32, u32, u32, wgpu::TextureFormat),
        jitter: [f32; 2],
        dt: f32,
        frame: u32,
        show_wireframe: bool,
        show_waterline_marker: bool,
        pvs: &[Option<OceanPvsTileMask>; OCEAN_MAX_SURFACES],
        sky_pvs_mask: u8,
        sky_faces: Option<[&wgpu::TextureView; 6]>,
        water_distance: &wgpu::TextureView,
        camera_water_surface: f32,
        sky_environment: Option<&::blue_water_sky::EnvironmentMap>,
        sky_light: Option<::blue_water_sky::CelestialLight>,
        aerial: Option<&::blue_water_render::AerialVolume>,
        cloud: Option<&::blue_water_clouds::CloudShadow>,
        volumes: &super::optics::Uniform,
        godot_receiver_color: Option<&wgpu::TextureView>,
        legacy_fog: Option<([f32; 4], [f32; 4])>,
        rain: f32,
    ) {
        if self.settings.backend == 3 {
            self.prepare_lakes(device, queue, encoder, camera, key, jitter, dt, frame, pvs,
                sky_faces, sky_environment, aerial, cloud);
            return;
        }
        let Some(blue) = self.blue_water.as_mut() else {
            return;
        };
        if self.promoted_surfaces.is_empty() {
            return;
        }
        let Some(renderer) = blue.original_surface.as_mut() else { return; };
        debug_assert_eq!(renderer.config, SurfaceConfig::new(key, self.settings));
        // Only present while Godot caustics can reach a receiver; otherwise the
        // surface reads its own scene copy (identical when no caustic applies).
        let surface_color = godot_receiver_color.unwrap_or(&renderer.scene.color);
        if show_wireframe {
            renderer.pipeline.ensure_wireframe(device, key.3, key.2);
        }
        if show_waterline_marker {
            renderer.pipeline.ensure_waterline_marker(device, key.3, key.2);
        }
        let next_external_sky = sky_environment.map(|sky| sky.view.clone());
        let external_sky_changed = renderer.external_sky != next_external_sky;
        renderer.external_sky = next_external_sky;
        let next_faces = sky_faces.map(|faces| faces.map(Clone::clone));
        let sky_changed = renderer.sky_faces != next_faces;
        if sky_changed {
            if let Some(faces) = sky_faces {
                let environment = renderer.map_environment.get_or_insert_with(|| {
                    blue_water_gpu::map_environment::MapEnvironment::new(device)
                });
                environment.bake(device, encoder, faces);
            }
            renderer.sky_faces = next_faces;
        }
        renderer.eye = [
            f64::from(camera.position.x / OCEAN_UNITS_PER_METER),
            f64::from(camera.position.z / OCEAN_UNITS_PER_METER),
        ];
        renderer.aerial = aerial.is_some();
        renderer.caustics.write(
            queue,
            [
                camera.position.x / OCEAN_UNITS_PER_METER,
                camera.position.z / OCEAN_UNITS_PER_METER,
            ],
        );
        for slot in renderer.slots.iter_mut().flatten() {
            slot.ready = false;
        }
        renderer.underwater_slot = None;
        let mut underwater_score = f32::INFINITY;
        // Blue Water's ship wake (`r_oceanWake 1`): created on demand, dropped again when switched off.
        if self.settings.blue_wake == 0 {
            if blue.wake.take().is_some() {
                blue.wake_pending.clear();
            }
        } else if blue.wake.is_none() {
            blue.wake = Some(super::blue_wake::BlueWake::new(
                device,
                blue_water_fx::wake::resolution_for(blue_water_fx::QualityTier::High),
            ));
        }
        if let Some(wake) = blue.wake.as_mut() {
            for command in blue.wake_pending.drain(..) {
                wake.command(device, command);
            }
        }
        let mut wake_stepped = false;
        for promoted in &self.promoted_surfaces {
            let index = promoted.descriptor.index;
            if index >= OCEAN_MAX_SURFACES {
                continue;
            }
            let source = promoted.descriptor.surface;
            // The Godot path gates sky continuation by the current camera's
            // BSP cluster. Merely having a sky contact on the map is NOT enough.
            let extension = (self.sky_extend_enabled
                && index < 8 && sky_pvs_mask & (1u8 << index) != 0)
                .then_some(self.sky_extend_regions[index])
                .flatten();
            // The original native draw previously fell back to an unrestricted
            // footprint when cluster_at returned None. Reuse Godot's explicit
            // all-zero visibility tiles to reject it before *any* native pass.
            if !Self::native_pool_visible(pvs[index], extension.is_some()) {
                continue;
            }
            // Blue Water's own clipmap follows the viewer continuously; the GPU
            // snaps each level to its own lattice (`Clipmap::frames`). Never
            // take the Godot mesh's centre, which is snapped to its coarse
            // outer ring and would move this whole clipmap in large jumps.
            let point = [camera.position.x, camera.position.z];
            let centre_units = if extension.is_some() {
                point
            } else {
                self.masks.closest_point(index, point).unwrap_or([
                    point[0].clamp(source.minimum[0], source.maximum[0]),
                    point[1].clamp(source.minimum[1], source.maximum[1]),
                ])
            };
            let centre = centre_units.map(|v| f64::from(v / OCEAN_UNITS_PER_METER));
            let distance = super::promoted_footprint_distance(
                camera.position.to_array(),
                source,
                &self.masks,
                index,
            ) / OCEAN_UNITS_PER_METER;
            let standoff = if extension.is_some() {
                (camera.position.y - source.plane_height).abs() / OCEAN_UNITS_PER_METER
            } else {
                distance
            };
            let detail = renderer.clipmap.clipmap().detail(f64::from(standoff));
            if !wake_stepped {
                wake_stepped = true;
                if let Some(wake) = blue.wake.as_mut().filter(|wake| wake.busy()) {
                    let map = renderer.clipmap.clipmap();
                    let forward = camera.forward();
                    wake.prepare(
                        device,
                        queue,
                        encoder,
                        f64::from(dt),
                        [
                            f64::from(camera.position.x / OCEAN_UNITS_PER_METER),
                            f64::from((camera.position.y - source.plane_height) / OCEAN_UNITS_PER_METER),
                            f64::from(camera.position.z / OCEAN_UNITS_PER_METER),
                        ],
                        [f64::from(forward.x), f64::from(forward.y), f64::from(forward.z)],
                        f64::from(camera.fov_y_for_viewport(key.0, key.1)),
                        f64::from(key.1),
                        blue.field.input_parameters().significant_height,
                        super::blue_wake::MeshDetail {
                            spacing: map.cell(detail.first) * (1.0 + f64::from(detail.morph)),
                            growth: map.cell(0) / map.half(0),
                        },
                    );
                }
            }
            let radius = if extension.is_some() {
                // The Godot coverage radius is measured from the Godot mesh's
                // snapped centre; re-measure it from this clipmap's centre.
                let godot_centre = self.promoted_centres[index].unwrap_or(centre_units);
                self.promoted_far_radii[index].unwrap_or(camera.far_distance())
                    + (godot_centre[0] - centre_units[0]).hypot(godot_centre[1] - centre_units[1])
            } else {
                (0..2)
                    .flat_map(|axis| {
                        [
                            source.minimum[axis] - centre_units[axis],
                            source.maximum[axis] - centre_units[axis],
                        ]
                    })
                    .map(f32::abs)
                    .fold(0.0, f32::max)
            } / OCEAN_UNITS_PER_METER;
            // Snapping displaces the outer frame by up to one cell.
            let outer = renderer
                .clipmap
                .outer_level_for_radius(f64::from(radius) * 1.05 + 1.0);
            let new_slot = renderer.slots[index].is_none();
            let slot = renderer.slots[index].get_or_insert_with(|| {
                let mut bindings = OriginalSurfaceBindings::with_foam(
                    device,
                    queue,
                    &renderer.pipeline,
                    renderer.foam.clone(),
                );
                bindings.set_scene(
                    device,
                    &renderer.pipeline,
                    surface_color,
                    &renderer.scene.depth,
                );
                let underwater = UnderwaterBindings::new(
                    device,
                    &renderer.underwater,
                    &bindings,
                    &renderer.scene.depth,
                    water_distance,
                );
                let caustic_receiver =
                    renderer
                        .caustics
                        .receiver(device, &bindings, &renderer.scene);
                SurfaceSlot {
                    rain: None,
                    bindings,
                    caustic_receiver,
                    underwater,
                    probe_due: true,
                    probe_active: false,
                    envelope: [0.0; 4],
                    wake_generation: 0,
                    first: 0,
                    outer: 0,
                    ring_steps: [1; 10],
                    fill_step: 1,
                    pruned: None,
                    ready: false,
                    horizon: false,
                }
            });
            if sky_changed || new_slot || external_sky_changed {
                if let Some(sky) = sky_environment {
                    slot.bindings.set_sky_environment(
                        device,
                        &renderer.pipeline,
                        &sky.view,
                        &sky.sampler,
                        &sky.uniforms,
                    );
                } else if renderer.sky_faces.is_some() {
                    let map = &renderer
                        .map_environment
                        .as_ref()
                        .expect("map sky environment")
                        .map;
                    slot.bindings.set_environment_atlas(
                        device,
                        &renderer.pipeline,
                        &map.view,
                        &map.sampler,
                    );
                } else {
                    slot.bindings.use_ambient_atlas(device, &renderer.pipeline);
                }
            }
            slot.bindings.set_atmosphere_resources(
                device,
                &renderer.pipeline,
                aerial.map(|air| (&air.volume_view, &air.sampler)),
                cloud.map(|cloud| (&cloud.view, &cloud.sampler, &cloud.uniforms)),
                surface_color,
                &renderer.scene.depth,
            );
            slot.bindings.set_scene(device, &renderer.pipeline, surface_color, &renderer.scene.depth);
            slot.bindings.set_wake(
                device,
                &renderer.pipeline,
                blue.wake.as_ref().map(|wake| {
                    let bound = wake.wake.bindings();
                    blue_water_gpu::original_surface::WakeResources {
                        uniform: bound.uniforms.clone(),
                        displays: [bound.displays[0].clone(), bound.displays[1].clone()],
                        trail_atlas: bound.trail_atlas.clone(),
                        torpedo_field: bound.torpedo_field.clone(),
                    }
                }),
            );
            if slot.wake_generation != slot.bindings.wake_generation() {
                slot.wake_generation = slot.bindings.wake_generation();
                slot.underwater = UnderwaterBindings::new(
                    device,
                    &renderer.underwater,
                    &slot.bindings,
                    &renderer.scene.depth,
                    water_distance,
                );
                slot.rain = None;
            }
            let mut boundary = SurfaceBoundary::zeroed();
            boundary.counts[3] = u32::from(self.settings.blue_water_boundary_mode);
            boundary.bounds = [
                source.minimum[0],
                source.minimum[1],
                source.maximum[0],
                source.maximum[1],
            ];
            boundary.scale = [
                OCEAN_UNITS_PER_METER,
                0.75,
                self.settings.pixel_detail_mode.min(5) as f32,
                self.settings.pixel_detail_distance.max(1.0),
            ];
            let first = self.masks.ranges[index][0] as usize * 2;
            let count = self.masks.ranges[index][1] as usize;
            assert!(count <= 256 && first + count * 2 <= self.masks.triangles.len());
            boundary.counts[0] = count as u32;
            boundary.tiles = self.masks.tiles[index];
            boundary.triangles[..count * 2]
                .copy_from_slice(&self.masks.triangles[first..first + count * 2]);
            if self.settings.blue_water_boundary_mode == 3 {
                boundary.prepare_candidates();
            }
            if let Some(sky) = extension {
                boundary.counts[1] = sky.count as u32;
                boundary.sky = sky.segments;
            } else if let Some(visible) = pvs[index] {
                boundary.counts[2] = 1;
                for (target, values) in boundary.pvs.iter_mut().zip(visible.chunks_exact(4)) {
                    target.copy_from_slice(values);
                }
            }
            slot.bindings.write_boundary(queue, &boundary);
            let mut surface_view = surface_camera(
                camera,
                source.plane_height,
                key.0,
                key.1,
                jitter,
                self.elapsed,
                dt,
                frame,
            );
            slot.horizon = extension.is_some() && self.sky_infinite_enabled;
            slot.bindings.write_camera(queue, &surface_view);
            let foam = super::linear_color(promoted.settings.foam_color);
            // The native foam shader already stretches its texture along
            // the spectrum's wind direction. Drive its existing uniform with
            // real spectrum wind rather than the upstream fixed 0.5 (2:1).
            // Foam opacity follows wave height separately: a perfectly flat
            // native sea cannot display static windrows or old whitecaps.
            // Shore/contact foam and ship wakes remain independent.
            let native_waves = promoted.settings.blue_water_waves;
            let foam_scale = native_foam_strength(native_waves);
            let mut appearance = self.settings.blue_water_optics.appearance();
            appearance.crest_foam = [foam[0], foam[1], foam[2], foam_scale];
            appearance.surface_foam = [foam[0], foam[1], foam[2], 0.6 * foam_scale];
            appearance.windrow_coverage *= foam_scale;
            appearance.wind_stretch = native_foam_stretch(native_waves.wind_speed);
            appearance.foam_streak_mean = renderer.foam.streak_mean;
            appearance.rain = rain;
            let mut prefix =
                renderer
                    .clipmap
                    .surface_prefix(centre, f64::from(standoff), appearance);
            // The upstream material owns aerial perspective for the water;
            // the earlier opaque-air pass only sees submerged receivers.
            prefix[20][3] = f32::from(aerial.is_some());
            if let Some((color_depth, params)) = legacy_fog {
                prefix[19] = color_depth;
                prefix[19][3] /= OCEAN_UNITS_PER_METER;
                prefix[20][0] = params[0];
                prefix[20][1] = params[1];
            }
            let point = [camera.position.x, camera.position.z];
            let closest = self.masks.closest_point(index, point).unwrap_or([
                point[0].clamp(source.minimum[0], source.maximum[0]),
                point[1].clamp(source.minimum[1], source.maximum[1]),
            ]);
            let horizontal =
                ((closest[0] - point[0]).powi(2) + (closest[1] - point[1]).powi(2)).sqrt();
            // Include the near plane's reach across an outline. The probe
            // still checks each ray against the exact GPU admission mask.
            let near_reach = camera.near_distance()
                * (1.0 + surface_view.tan_half_fov_y * (1.0 + surface_view.aspect.powi(2)).sqrt());
            let analytic_height = f64::from(super::hybrid_swell_uniform(&promoted.settings, self.elapsed)[0][3]);
            let crest_bound = (2.0 * blue.field.input_parameters().significant_height + analytic_height + 8.1) as f32;
            let near_surface = surface_view.position_time[1]
                <= crest_bound + near_reach / OCEAN_UNITS_PER_METER
                && (horizontal <= near_reach || extension.is_some_and(|sky| sky.contains(point)));
            slot.probe_due = new_slot || near_surface || slot.probe_active;
            slot.probe_active = near_surface;
            slot.envelope = [
                source.plane_height,
                crest_bound * OCEAN_UNITS_PER_METER,
                0.0,
                0.0,
            ];
            prefix[23][3] = if near_surface { 1.0 } else { 0.0 };
            // High tier's original 16-step reflection trace reads the same
            // opaque color and reversed depth as surface transmission.
            prefix[22] = [
                if self.settings.blue_water_ssr { 1.0 } else { 0.0 },
                400.0,
                self.settings.blue_water_refraction,
                self.settings.blue_water_refraction_mode as f32,
            ];
            // The selected mesh has fewer actual triangles at every native
            // ring; reuse the upstream fill/ring morph for crack-free joins.
            let first = detail.first;
            slot.bindings.write_surface_prefix(queue, &prefix);
            slot.bindings.write_hybrid_swell(queue,
                &super::hybrid_swell_uniform(&promoted.settings, self.elapsed));
            let sun = self.environment.sun_direction_intensity;
            let tint = self.environment.sun_color;
            let (direction, color) = sky_light.map_or(
                ([-sun[0], -sun[1], -sun[2]], [tint[0], tint[1], tint[2]]),
                |light| (light.direction.to_array(), light.color.to_array()),
            );
            let intensity = sky_light.map_or(sun[3] * 5.0, |light| light.intensity);
            slot.caustic_receiver.write(
                queue,
                &blue_water_gpu::caustics::ReceiverParams {
                    sun: [
                        direction[0],
                        direction[1],
                        direction[2],
                        if self.settings.blue_water_caustics_mode >= 2 { 0.0 } else { self.settings.optics.caustics },
                    ],
                    light: [color[0], color[1], color[2], intensity],
                    plane: [
                        source.plane_height,
                        volumes.params[3],
                        OCEAN_UNITS_PER_METER,
                        1.0 - ((surface_view.position_time[1] - 12.0) / 13.0).clamp(0.0, 1.0),
                    ],
                    absorption: [
                        appearance.absorption[0],
                        appearance.absorption[1],
                        appearance.absorption[2],
                        0.0,
                    ],
                    minimum: volumes.minimum,
                    maximum: volumes.maximum,
                },
            );
            slot.underwater.write(
                queue,
                &surface_view,
                appearance,
                direction,
                color,
                near_surface,
            );
            let score = if (source.plane_height - camera_water_surface).abs() < 0.75 {
                0.0
            } else {
                (camera.position.y - source.plane_height).abs() + 1.0
            };
            if near_surface && score < underwater_score {
                underwater_score = score;
                renderer.underwater_slot = Some(index);
            }
            if sky_environment.is_none() {
                slot.bindings
                    .write_sun(queue, direction, color, sun[3] * 5.0);
            }
            let ambient = self.environment.sky_ambient;
            slot.bindings
                .write_ambient(queue, [ambient[0], ambient[1], ambient[2]]);
            slot.first = first;
            slot.outer = outer.max(first);
            slot.ring_steps = projected_ring_steps(
                renderer.clipmap.clipmap(), first,
                (camera.position.y - source.plane_height) / OCEAN_UNITS_PER_METER,
                key.1, (camera.fov_y_for_viewport(key.0, key.1) * 0.5).tan(),
                self.settings.screen_lod_pixels,
            );
            slot.fill_step = 1;
            if self.settings.adaptive_mesh {
                let analytic = super::hybrid_swell_uniform(&promoted.settings, self.elapsed)[0];
                let waves = self.settings.blue_water_waves;
                // Only the FFT component can drive the FFT curvature bound;
                // at a pure Gerstner blend use the analytic curvature instead.
                let fft_weight = if self.settings.backend == 2 {
                    self.settings.hybrid_fft_blend.clamp(0.0, 1.0)
                } else { 1.0 };
                let (steps, fill_step) = curvature_mesh_steps(
                    renderer.clipmap.clipmap(), first,
                    (camera.position.y - source.plane_height) / OCEAN_UNITS_PER_METER,
                    key.1, (camera.fov_y_for_viewport(key.0, key.1) * 0.5).tan(),
                    waves.height * fft_weight, waves.wavelength,
                    waves.choppiness, waves.wind_speed, analytic,
                );
                // Curvature drives all drawn rings including the nearest
                // three. The independent screen LOD is used only when this
                // experiment is Off, for an unambiguous A/B comparison.
                slot.ring_steps = steps;
                slot.fill_step = fill_step;
            }
            // The Godot clipmap never uploads dry/PVS-invisible cells. Use
            // exactly the same conservative rectangle admission for Blue
            // Water's native indexed topology, while keeping all original
            // vertices, ring morphing, wave simulation and material intact.
            // Blue Water's frames are metres; the Godot masks use JKA units.
            // The finest levels re-snap every fraction of a metre, but the
            // fragment mask only matters at footprint edges. Prune from frames
            // quantized to PRUNE_QUANTUM_METRES and pad every test by it, so one
            // prune stays valid across a ~16 m move instead of rebuilding (and
            // re-uploading every index) each time the finest lattice crosses.
            const PRUNE_QUANTUM_METRES: f32 = 16.0;
            let frames: [[f32; 4]; 10] = std::array::from_fn(|level| {
                let frame = prefix[level];
                if frame[2] * 2.0 >= PRUNE_QUANTUM_METRES {
                    return frame;
                }
                let snap = |v: f32| (v / PRUNE_QUANTUM_METRES).round() * PRUNE_QUANTUM_METRES;
                [snap(frame[0]), snap(frame[1]), frame[2], frame[3]]
            });
            let key = NativePruneKey {
                centres: frames.map(|frame| [frame[0].to_bits(), frame[1].to_bits()]),
                first: slot.first,
                outer: slot.outer,
                steps: slot.ring_steps,
                fill_step: slot.fill_step,
                horizon: slot.horizon,
                sky_extension: extension.is_some(),
                pvs: if extension.is_none() { pvs[index] } else { None },
            };
            let geometry = slot.pruned.get_or_insert_with(|| NativePrunedGeometry {
                buffer: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Blue Water Godot-style pruned pool indices"),
                    size: (renderer.clipmap.pruned_index_capacity() * std::mem::size_of::<u32>()) as u64,
                    usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                count: 0,
                baseline_count: None,
                upload_generation: 0,
                key: None,
            });
            if geometry.key.as_ref() != Some(&key) {
                let indices = renderer.clipmap.pruned_indices_with_fill(
                    slot.first, slot.outer, &slot.ring_steps, slot.fill_step, slot.horizon,
                    &frames, PRUNE_QUANTUM_METRES,
                    |lo, hi| {
                        let lo = lo.map(|v| v * OCEAN_UNITS_PER_METER);
                        let hi = hi.map(|v| v * OCEAN_UNITS_PER_METER);
                        super::promoted_ocean_cell_may_contain(
                            source, &self.masks, Some(index),
                            if extension.is_some() { None } else { pvs[index].as_ref() },
                            extension.as_ref(), lo, hi,
                        )
                    },
                );
                debug_assert!(indices.len() <= renderer.clipmap.pruned_index_capacity());
                if !indices.is_empty() {
                    queue.write_buffer(&geometry.buffer, 0, bytemuck::cast_slice(&indices));
                }
                geometry.count = indices.len() as u32;
                geometry.baseline_count = None; // PVS, ring LOD or fill changed.
                geometry.upload_generation += 1;
                geometry.key = Some(key);
            }
            // Measure the FULL-DETAIL reference using the identical snapped
            // frames, PVS mask and admission predicate as the actual index upload.
            // Debug-only; cached until the selected topology key changes.
            if self.settings.adaptive_mesh_debug && geometry.baseline_count.is_none() {
                let baseline = renderer.clipmap.pruned_indices_with_fill(
                    slot.first, slot.outer, &[1u8; 10], 1, slot.horizon,
                    &frames, PRUNE_QUANTUM_METRES,
                    |lo, hi| {
                        let lo = lo.map(|v| v * OCEAN_UNITS_PER_METER);
                        let hi = hi.map(|v| v * OCEAN_UNITS_PER_METER);
                        super::promoted_ocean_cell_may_contain(
                            source, &self.masks, Some(index),
                            if extension.is_some() { None } else { pvs[index].as_ref() },
                            extension.as_ref(), lo, hi,
                        )
                    },
                );
                geometry.baseline_count = Some(baseline.len() as u32);
            }
            slot.ready = true;
        }
    }

    /// Debug counts reflect exactly the ready native slots admitted by the
    /// unified water pass, including sky-horizon continuation draws.
    pub fn original_adaptive_mesh_counts(&self, admitted: u8) -> (u64, u64, u32, u32) {
        let Some(renderer) = self.blue_water.as_ref().and_then(|blue| blue.original_surface.as_ref())
            else { return (0, 0, 0, 0); };
        let mut baseline = 0u64;
        let mut selected = 0u64;
        let mut counted = 0u32;
        let mut missing = 0u32;
        for (index, slot) in renderer.slots.iter().enumerate() {
            let Some(slot) = slot.as_ref().filter(|slot| slot.ready &&
                (admitted >> index & 1 != 0 || slot.horizon)) else { continue; };
            let Some(mesh) = slot.pruned.as_ref() else { missing += 1; continue; };
            if let Some(full_count) = mesh.baseline_count {
                baseline += u64::from(full_count) / 3;
                selected += u64::from(mesh.count) / 3;
                counted += 1;
            } else { missing += 1; }
        }
        (baseline, selected, counted, missing)
    }

    /// Explain which strides really made it into the native slot; also expose
    /// buffer upload generation so a toggled cvar with no upload is evident.
    pub fn original_adaptive_mesh_steps(&self) -> String {
        let Some(renderer) = self.blue_water.as_ref().and_then(|blue| blue.original_surface.as_ref())
            else { return "no native original-surface renderer".to_owned(); };
        let mut out = String::new();
        for (index, slot) in renderer.slots.iter().enumerate() {
            let Some(slot) = slot.as_ref().filter(|slot| slot.ready) else { continue; };
            let generation = slot.pruned.as_ref().map_or(0, |geometry| geometry.upload_generation);
            let active = &slot.ring_steps[slot.first..=slot.outer];
            out.push_str(&format!(" pool{index}[rings={}-{} fill={} steps={active:?} upload={generation}]",
                slot.first, slot.outer, slot.fill_step));
        }
        if out.is_empty() { "no ready native water pools".to_owned() } else { out }
    }

    /// True once the native pipeline owns the surface draw, even if a pool
    /// was rejected by this frame's PVS. `original_surface_triangles(None)`
    /// must never trigger a fallback to the old Godot indices in that case.
    pub fn original_surface_prepared(&self) -> bool {
        if self.settings.backend == 3 { return self.lakes.is_some(); }
        self.blue_water.as_ref().is_some_and(|blue| blue.original_surface.is_some())
    }

    pub fn original_surface_triangles(&self, index: usize) -> Option<u64> {
        if self.settings.backend == 3 { return self.lakes.as_ref()?.triangles(index); }
        let renderer = self.blue_water.as_ref()?.original_surface.as_ref()?;
        let slot = renderer.slots.get(index)?.as_ref()?;
        slot.ready.then(|| {
            slot.pruned.as_ref().map_or(0, |mesh| u64::from(mesh.count) / 3)
        })
    }

    pub fn apply_original_caustics(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) {
        if let Some(renderer) = self
            .blue_water
            .as_ref()
            .and_then(|blue| blue.original_surface.as_ref())
        {
            let blue = self.blue_water.as_ref().expect("original ocean");
            if renderer
                .slots
                .iter()
                .flatten()
                .any(|slot| slot.ready && slot.caustic_receiver.enabled)
            {
                renderer.scene.capture(device, encoder, color, depth);
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Original caustics on JKA opaque receivers"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: color,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        ..Default::default()
                    });
                    for (index, slot) in renderer.slots.iter().enumerate().filter_map(|(i,s)|s.as_ref().filter(|s|s.ready).map(|s|(i,s))) {
                        renderer.caustics.draw(
                            &mut pass,
                            &slot.caustic_receiver,
                            blue.waves(index).group(&blue.field),
                        );
                    }
                }
            }
        }
    }

    /// Sample the same native FFT and admitted footprints used by Hybrid's sea.
    pub fn prepare_original_rain_heights(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        weather_eye: [f32; 3],
    ) -> Option<wgpu::TextureView> {
        let blue = self.blue_water.as_mut()?;
        let renderer = blue.original_surface.as_mut()?;
        let heights = renderer.rain_heights.get_or_insert_with(||
            blue_water_gpu::rain_heights::RainHeights::new(device, blue.surface_waves.layout()));
        heights.begin(queue, encoder, weather_eye);
        for (index, slot) in renderer.slots.iter_mut().enumerate().filter_map(|(i,s)|s.as_mut().filter(|s|s.ready).map(|s|(i,s))) {
            let read = slot.rain.get_or_insert_with(|| heights.surface(device, &slot.bindings));
            heights.encode(encoder, read, blue.shore_waves[index].as_ref().unwrap_or(&blue.surface_waves).group(&blue.field));
        }
        Some(heights.view.clone())
    }

    /// Whether native spindrift would draw this frame (`r_oceanSpindrift`, wind at a gale's onset).
    pub fn original_spindrift_active(&self) -> bool {
        self.settings.spindrift != 0
            && self.blue_water.as_ref().is_some_and(|blue| {
                blue.original_surface.as_ref().is_some_and(|renderer| renderer.slots.iter().flatten().any(|slot| slot.ready))
                    && blue_water_fx::spindrift::spindrift_strength(blue.field.input_parameters().wind_speed) > 0.0
            })
    }

    /// Native Blue Water spindrift over every admitted pool, in its own pass over the finished scene (after water and
    /// transparents), fading into the sea through a fresh single-sample copy of colour and depth.
    pub fn draw_original_spindrift(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        resolve: Option<&wgpu::TextureView>,
        depth: &wgpu::TextureView,
        flash: f32,
    ) -> bool {
        if !self.original_spindrift_active() {
            return false;
        }
        let elapsed = f64::from(self.elapsed);
        let Some(blue) = self.blue_water.as_mut() else { return false; };
        let parameters = blue.field.input_parameters();
        let Some(renderer) = blue.original_surface.as_mut() else { return false; };
        let key = renderer.key;
        if renderer.spindrift.is_none() {
            renderer.spindrift = Some(blue_water_fx::spindrift::Spindrift::new(
                device,
                &renderer.pipeline.camera_layout,
                blue.surface_waves.layout(),
                &renderer.pipeline.environment_layout,
                key.3,
                key.2,
            ));
        }
        renderer.scene.capture(device, encoder, color, depth);
        let frame = blue_water_fx::spindrift::SpindriftFrame {
            wind_speed: parameters.wind_speed,
            wind_direction: parameters.wind_direction,
            time: elapsed,
            significant_height: parameters.significant_height,
            flash,
            eye: renderer.eye,
            aerial: renderer.aerial,
        };
        let pools: Vec<_> = renderer
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i,s)|s.as_ref().filter(|s|s.ready).map(|s|(i,s)))
            .map(|(index,slot)| blue_water_fx::spindrift::PoolDraw {
                waves: Some(blue.shore_waves[index].as_ref().unwrap_or(&blue.surface_waves).group(&blue.field)),
                camera: &slot.bindings.camera_group,
                boundary: slot.bindings.boundary(),
                cloud: slot.bindings.cloud_resources(),
                environment: &slot.bindings.environment_group,
            })
            .collect();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("native Blue Water spindrift"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: resolve,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        renderer.spindrift.as_ref().expect("spindrift").draw(
            device,
            queue,
            &mut pass,
            &frame,
            &renderer.scene.depth,
            (&renderer.foam.view, &renderer.foam.sampler),
            blue.surface_waves.group(&blue.field),
            &pools,
        ) > 0
    }

    /// Capture the lit scene after opaque caustics and aerial lighting.
    pub fn capture_original_scene(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) {
        if let Some(lakes) = &self.lakes {
            lakes.scene.capture(device, encoder, color, depth);
            return;
        }
        if let Some(renderer) = self
            .blue_water
            .as_ref()
            .and_then(|blue| blue.original_surface.as_ref())
        {
            renderer.scene.capture(device, encoder, color, depth);
        }
    }

    /// Caustics A/B: 0 original full-rate, 1 only freshly published FFT data,
    /// 2 off. Mode 1 follows wave steps and can lag the camera-centered texture.
    pub fn prepare_original_caustics(&self, encoder: &mut wgpu::CommandEncoder) {
        if self.settings.blue_water_caustics_mode >= 2 { return; }
        if let Some(blue) = &self.blue_water {
            if let Some(renderer) = &blue.original_surface {
                if self.settings.blue_water_caustics_mode == 0 || blue.updated_this_frame {
                    if renderer.slots.iter().flatten().any(|slot| slot.ready && slot.caustic_receiver.enabled) {
                        let index=renderer.underwater_slot.or_else(||renderer.slots.iter().position(|s|s.as_ref().is_some_and(|s|s.ready && s.caustic_receiver.enabled))).unwrap_or(0);
                        renderer.caustics.encode(encoder, blue.waves(index).group(&blue.field));
                    }
                }
            }
        }
    }

    /// Underwater ray probe, independent of caustic FFT gating.
    pub fn prepare_original_probe(&self, encoder: &mut wgpu::CommandEncoder) {
        if let Some(blue) = &self.blue_water {
            if let Some(renderer) = &blue.original_surface {
                for (index, slot) in renderer.slots.iter().enumerate()
                    .filter_map(|(i,s)|s.as_ref().filter(|s|s.ready && s.probe_due).map(|s|(i,s)))
                {
                    slot.underwater.probe(
                        encoder,
                        &renderer.underwater,
                        blue.waves(index).group(&blue.field),
                    );
                }
            }
        }
    }
    pub fn original_underwater_probe(&self) -> Option<&wgpu::TextureView> {
        let renderer = self.blue_water.as_ref()?.original_surface.as_ref()?;
        let slot = renderer
            .underwater_slot
            .and_then(|index| renderer.slots[index].as_ref())?;
        Some(slot.bindings.underwater_probe())
    }

    pub fn original_underwater_active(&self) -> bool {
        self.blue_water
            .as_ref()
            .and_then(|blue| blue.original_surface.as_ref())
            .is_some_and(|renderer| renderer.underwater_slot.is_some())
    }

    pub fn original_underwater_envelope(&self) -> [f32; 4] {
        self.blue_water
            .as_ref()
            .and_then(|blue| blue.original_surface.as_ref())
            .and_then(|renderer| {
                renderer
                    .underwater_slot
                    .and_then(|index| renderer.slots[index].as_ref())
            })
            .map_or([0.0; 4], |slot| slot.envelope)
    }

    pub fn draw_original_underwater(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        resolve: Option<&wgpu::TextureView>,
        depth: &wgpu::TextureView,
    ) {
        let Some(renderer) = self
            .blue_water
            .as_ref()
            .and_then(|blue| blue.original_surface.as_ref())
        else {
            return;
        };
        let Some(slot) = renderer
            .underwater_slot
            .and_then(|index| renderer.slots[index].as_ref())
        else {
            return;
        };
        // Refresh after water and transparency. No effect samples a live attachment.
        renderer.scene.capture(device, encoder, color, depth);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Blue Water original underwater effects"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: resolve,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        slot.underwater.draw(&mut pass, &renderer.underwater, self.settings.blue_water_underwater_mode != 1);
    }

    /// Every native rendering pass uses the same *pruned* index buffer;
    /// otherwise r_showtris, the full material and reduced shading disagree.
    fn draw_native_slot<'a>(
        renderer: &'a SurfaceRenderer,
        pass: &mut wgpu::RenderPass<'a>,
        slot: &'a SurfaceSlot,
    ) {
        if let Some(geometry) = slot.pruned.as_ref() {
            renderer.clipmap.draw_pruned(pass, &geometry.buffer, geometry.count);
        }
    }

    /// r_showtris OCEAN: draw the REAL Blue Water clipmap, not the host's
    /// Godot mesh. Uses the same admitted surfaces and selected first/outer LOD.
    pub fn draw_original_wireframe<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, admitted: u8) {
        let Some(blue) = self.blue_water.as_ref() else { return; };
        let Some(renderer) = blue.original_surface.as_ref() else { return; };
        let Some(pipeline) = renderer.pipeline.wireframe.as_ref() else { return; };
        for (index, slot) in renderer.slots.iter().enumerate() {
            let Some(slot) = slot.as_ref().filter(|slot|
                slot.ready && (admitted >> index & 1 != 0 || slot.horizon)) else { continue; };
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &slot.bindings.camera_group, &[]);
            pass.set_bind_group(1, &slot.bindings.material_group, &[]);
            pass.set_bind_group(2, blue.waves(index).group(&blue.field), &[]);
            pass.set_bind_group(3, &slot.bindings.environment_group, &[]);
            Self::draw_native_slot(renderer, pass, slot);
        }
    }

    /// Mode 3: native GPU wave data sampled at CAMERA XZ and shown 100 units ahead,
    /// even if Hybrid currently selects the Godot submerged surface.
    pub fn draw_original_waterline_marker<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) -> u32 {
        let Some(blue) = &self.blue_water else { return 0; };
        let Some(renderer) = &blue.original_surface else { return 0; };
        let Some(pipeline) = &renderer.pipeline.waterline_marker else { return 0; };
        let mut count = 0;
        for (index, slot) in renderer.slots.iter().enumerate().filter_map(|(i,s)|s.as_ref().filter(|s|s.ready).map(|s|(i,s))) {
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &slot.bindings.camera_group, &[]);
            pass.set_bind_group(1, &slot.bindings.material_group, &[]);
            pass.set_bind_group(2, blue.waves(index).group(&blue.field), &[]);
            pass.set_bind_group(3, &slot.bindings.environment_group, &[]);
            pass.draw(0..36, 0..1);
            count += 1;
        }
        count
    }

    /// Execute the expensive *original* material once per coarse pixel.
    /// Subsequent full-resolution geometry uses a cheap upscaler fragment.
    pub fn encode_reduced_original_surface(&self, encoder: &mut wgpu::CommandEncoder, admitted: u8) {
        let Some(blue) = self.blue_water.as_ref() else { return; };
        let Some(renderer) = blue.original_surface.as_ref() else { return; };
        let (Some(target), Some(pipeline)) = (renderer.reduced.as_ref(), renderer.pipeline.reduced_pipeline.as_ref()) else { return; };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Blue Water original material / true reduced pixels"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.color_view, depth_slice: None, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &target.depth_view,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        for (index, slot) in renderer.slots.iter().enumerate() {
            let Some(slot) = slot.as_ref().filter(|s| s.ready && (admitted >> index & 1 != 0 || s.horizon)) else { continue; };
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &slot.bindings.camera_group, &[]);
            pass.set_bind_group(1, &slot.bindings.material_group, &[]);
            pass.set_bind_group(2, blue.waves(index).group(&blue.field), &[]);
            pass.set_bind_group(3, &slot.bindings.environment_group, &[]);
            Self::draw_native_slot(renderer, &mut pass, slot);
        }
    }

    pub fn draw_original_near_detail<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, admitted: u8) {
        let Some(blue) = self.blue_water.as_ref() else { return; };
        let Some(renderer) = blue.original_surface.as_ref() else { return; };
        let Some(pipeline) = renderer.pipeline.near_pipeline.as_ref() else { return; };
        for (index, slot) in renderer.slots.iter().enumerate() {
            let Some(slot) = slot.as_ref().filter(|s| s.ready && (admitted >> index & 1 != 0 || s.horizon)) else { continue; };
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &slot.bindings.camera_group, &[]);
            pass.set_bind_group(1, &slot.bindings.material_group, &[]);
            pass.set_bind_group(2, blue.waves(index).group(&blue.field), &[]);
            pass.set_bind_group(3, &slot.bindings.environment_group, &[]);
            Self::draw_native_slot(renderer, pass, slot);
        }
    }

    pub fn draw_original_surfaces<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        admitted: u8,
    ) -> (u32, u32, u64) {
        if let Some(lakes) = &self.lakes { return lakes.draw(pass, admitted); }
        let Some(blue) = self.blue_water.as_ref() else {
            return (0, 0, 0);
        };
        let Some(renderer) = blue.original_surface.as_ref() else {
            return (0, 0, 0);
        };
        let mut drawn = 0;
        let mut horizon_only = 0;
        let mut horizon_triangles = 0;
        for (index, slot) in renderer.slots.iter().enumerate() {
            // A qualified distant horizon remains visible when its nearby
            // finite mesh is outside the frustum. The sky/PVS gate was applied
            // when selecting this slot's sky contacts, before preparation.
            let Some(slot) = slot
                .as_ref()
                .filter(|slot| slot.ready && (admitted >> index & 1 != 0 || slot.horizon))
            else {
                continue;
            };
            if admitted >> index & 1 == 0 {
                horizon_only += 1;
                horizon_triangles += slot.pruned.as_ref().map_or(0, |mesh| u64::from(mesh.count) / 3);
            }
            pass.set_pipeline(renderer.pipeline.reconstruct_pipeline.as_ref()
                .unwrap_or(&renderer.pipeline.pipeline));
            pass.set_bind_group(0, &slot.bindings.camera_group, &[]);
            pass.set_bind_group(1, &slot.bindings.material_group, &[]);
            pass.set_bind_group(2, blue.waves(index).group(&blue.field), &[]);
            pass.set_bind_group(3, &slot.bindings.environment_group, &[]);
            if let Some(target) = renderer.reduced.as_ref() {
                pass.set_bind_group(4, &target.binding, &[]);
            }
            Self::draw_native_slot(renderer, pass, slot);
            drawn += 1;
        }
        (drawn, horizon_only, horizon_triangles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_pool_visibility_matches_godot_pvs_semantics() {
        // PVS absent (or source without indexed water faces) conservatively
        // admits a pool. A compiled PVS with no visible water tiles does not.
        assert!(OceanGpu::native_pool_visible(None, false));
        assert!(!OceanGpu::native_pool_visible(Some([0; crate::ocean::OCEAN_PRUNE_TILE_AXIS]), false));
        let mut partial = [0; crate::ocean::OCEAN_PRUNE_TILE_AXIS];
        partial[12] = 1 << 8;
        assert!(OceanGpu::native_pool_visible(Some(partial), false));
        assert!(OceanGpu::native_pool_visible(Some([0; crate::ocean::OCEAN_PRUNE_TILE_AXIS]), true));
    }

    #[test]
    fn metre_camera_matches_jka_projection_at_each_water_plane() {
        let mut camera = crate::camera::Camera::new([1200.0, 900.0, -600.0], 0.7);
        camera.pitch = -0.2;
        let jitter = [0.375, -0.25];
        let host = camera.view_projection_jittered(1280, 720, jitter);
        for plane in [-1024.0, 0.0, 800.0] {
            let uniform = surface_camera(&camera, plane, 1280, 720, jitter, 0.0, 0.016, 3);
            let projection = Mat4::from_cols_array_2d(&uniform.view_projection);
            for point in [
                Vec3::new(900.0, plane, -1600.0),
                Vec3::new(2000.0, plane + 16.0, 0.0),
            ] {
                let a = host * point.extend(1.0);
                let b =
                    projection * ((point - Vec3::Y * plane) / OCEAN_UNITS_PER_METER).extend(1.0);
                let a = a.truncate() / a.w;
                let b = b.truncate() / b.w;
                assert!(
                    (a - b).abs().max_element() < 2e-5,
                    "plane {plane}: {a:?} != {b:?}"
                );
            }
        }
    }
}

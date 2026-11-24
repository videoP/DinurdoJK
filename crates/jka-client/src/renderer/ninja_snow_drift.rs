//! Fluid Ninja Live 2 full Snow FVOL 200 m passive snowdust.
//! The FVOL Niagara producer and material are distinct from SVOL and the
//! particle blizzard. A BSP snow-height atlas is an explicit Landscape bridge.
//! No resources or pipelines exist when ninja_composite is disabled.
use super::{AssetSearchPath, Renderer, Vec2};
use super::environment::snow::{SnowSource, spawn_snow_shell_job};
use crate::surface_deformation::SNOW_SHELL_CHUNK_SIZE;
use bytemuck::{Pod, Zeroable};
use jka_ninja::{FogImage, SnowProfile};
use std::sync::{Arc, mpsc};

const RES: usize = 128; // SurfaceAlignedVolumes: User.SampleGridResolution=128.
const EXTENT_MAP_UNITS: f32 = 200.0 * 100.0 / 2.54; // User.EmitterExtents: 200m.
const SOURCE_IMAGE: FogImage = FogImage {
    qpath: "textures/japro/t_noisetemplate1.png",
    size: [1024, 1024, 0],
    srgb: true,
};
const VOLUME_IMAGE: FogImage = FogImage {
    qpath: "textures/japro/fog/volume_noise2.dds",
    size: [128, 128, 128],
    srgb: false,
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SnowDustUniform {
    centre_extent: [f32; 4],
    height_origin: [f32; 4],
    time_sky: [f32; 4],
}

pub(in crate::renderer) struct NinjaSnowDustState {
    source: Option<Arc<SnowSource>>,
    ready: Option<Box<Gpu>>,
    failed: bool,
    active: bool,
}
impl Default for NinjaSnowDustState {
    fn default() -> Self {
        Self { source: None, ready: None, failed: false, active: false }
    }
}

struct HeightBuild {
    left: Vec2,
    rgba: Vec<f32>,
}
struct Gpu {
    layout: wgpu::BindGroupLayout,
    solo: wgpu::ComputePipeline,
    combined: Option<wgpu::ComputePipeline>,
    group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    _height: wgpu::Texture,
    height_at: Option<Vec2>,
    job: Option<mpsc::Receiver<HeightBuild>>,
}

impl NinjaSnowDustState {
    pub(in crate::renderer) fn pass(&self, with_fog: bool) -> Option<(&wgpu::ComputePipeline, &wgpu::BindGroup)> {
        if !self.active { return None; }
        let gpu = self.ready.as_ref()?;
        Some((if with_fog { gpu.combined.as_ref()? } else { &gpu.solo }, &gpu.group))
    }
}

impl Renderer {
    pub(in crate::renderer) fn prepare_ninja_snow_drift(&mut self, draw_world: bool, fog_active: bool) -> bool {
        let available = draw_world && jka_ninja::INCLUDED
            && self.surface_deformation.mode().ninja_profile() == Some(SnowProfile::Composite)
            && self.world.as_ref().is_some_and(|w| w.snow_shell.has_snow() && w.froxel_bind_group.is_some());
        if !available {
            self.ninja_snow_drift = NinjaSnowDustState::default();
            return false;
        }
        let source = self.world.as_ref().expect("snow world").snow_shell.source.clone();
        if self.ninja_snow_drift.source.as_ref().is_none_or(|old| !Arc::ptr_eq(old, &source)) {
            self.ninja_snow_drift = NinjaSnowDustState::default();
            self.ninja_snow_drift.source = Some(source.clone());
        }
        self.ninja_snow_drift.active = false;
        if self.ninja_snow_drift.failed { return false; }
        if self.ninja_snow_drift.ready.is_none() {
            match Gpu::new(self) {
                Ok(gpu) => self.ninja_snow_drift.ready = Some(Box::new(gpu)),
                Err(e) => {
                    log::warn!("[NINJA SNOW] passive FVOL unavailable ({e})");
                    self.ninja_snow_drift.failed = true;
                    return false;
                }
            }
        }
        // The renderer's existing weather froxels remain the integration
        // authority for sun shadows, local light clusters and precipitation.
        let fog_layout = if fog_active { self.ninja_fog.layout() } else { None };
        let Some(gpu) = self.ninja_snow_drift.ready.as_mut() else { return false; };
        if fog_active && gpu.combined.is_none() {
            let Some(fog_layout) = fog_layout else { return false; };
            match gpu.ensure_combined(&self.device, &self.weather.fog.resources.layout, fog_layout) {
                Ok(()) => {},
                Err(e) => {
                    log::warn!("[NINJA SNOW] cannot combine snowdust and fog ({e})");
                    self.ninja_snow_drift.failed = true;
                    return false;
                }
            }
        }
        let focus = self.surface_deformation.field_center();
        if !focus.is_finite() { return false; }
        let snap = EXTENT_MAP_UNITS / RES as f32;
        let left = ((focus - Vec2::splat(0.5 * EXTENT_MAP_UNITS)) / snap).floor() * snap;
        // Poll a worker; never rasterize tens of thousands of terrain samples
        // on the frame-critical render thread. Keep the previous atlas until
        // the next world-space placement is uploaded, avoiding edge popping.
        if let Some(job) = &gpu.job {
            if let Ok(build) = job.try_recv() {
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo { texture: &gpu._height, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    bytemuck::cast_slice(&build.rgba),
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(RES as u32 * 16), rows_per_image: Some(RES as u32) },
                    wgpu::Extent3d { width: RES as u32, height: RES as u32, depth_or_array_layers: 1 },
                );
                gpu.height_at = Some(build.left);
                gpu.job = None;
            }
        }
        if gpu.job.is_none() && gpu.height_at != Some(left) {
            let (tx, rx) = mpsc::channel();
            let src = source;
            spawn_snow_shell_job(move || { let _ = tx.send(build_snow_heights(&src, left)); });
            gpu.job = Some(rx);
        }
        let Some(at) = gpu.height_at else { return false; };
        let sky = self.world.as_ref().map_or([0.0; 3], |w| w.sky_average);
        let uniform = SnowDustUniform {
            centre_extent: [at.x + 0.5 * EXTENT_MAP_UNITS, at.y + 0.5 * EXTENT_MAP_UNITS, EXTENT_MAP_UNITS, 0.0],
            height_origin: [at.x, at.y, EXTENT_MAP_UNITS, 0.0],
            time_sky: [self.weather_server_time_ms.unwrap_or_default() as f32 * 0.001, sky[0], sky[1], sky[2]],
        };
        self.queue.write_buffer(&gpu.uniform, 0, bytemuck::bytes_of(&uniform));
        self.ninja_snow_drift.active = true;
        true
    }
}

impl Gpu {
    fn new(renderer: &Renderer) -> Result<Self, String> {
        let device = &renderer.device;
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let result = Self::create(renderer);
        let validation = jka_river::river_ninja::checked_scope(validation);
        let memory = jka_river::river_ninja::checked_scope(memory);
        validation.and(memory)?;
        result
    }
    fn create(renderer: &Renderer) -> Result<Self, String> {
        let device = &renderer.device;
        let queue = &renderer.queue;
        let mut assets = AssetSearchPath::open_game(&renderer.base, renderer.detail_texture_game.as_deref())
            .map_err(|e| e.to_string())?;
        // Fail closed: the upstream sprites/noise are required, not replaced
        // by stock weather's random textures.
        let source = super::ninja_fog::image_view(&mut assets, device, queue, SOURCE_IMAGE)?;
        let noise = super::ninja_fog::image_view(&mut assets, device, queue, VOLUME_IMAGE)?;
        let height = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Ninja FVOL authored BSP snow height raster"),
            size: wgpu::Extent3d { width: RES as u32, height: RES as u32, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let height_view = height.create_view(&wgpu::TextureViewDescriptor::default());
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ninja FVOL snowdust parameters"),
            size: std::mem::size_of::<SnowDustUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let texture = |binding, dim, filterable| wgpu::BindGroupLayoutEntry {
            binding, visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: dim, multisampled: false,
            }, count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ninja FVOL snowdust layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                texture(1, wgpu::TextureViewDimension::D2, true),
                texture(2, wgpu::TextureViewDimension::D2, false),
                texture(3, wgpu::TextureViewDimension::D3, true),
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Ninja passive snow noise repeat"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Ninja passive snowdust material"), layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&source) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&height_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&noise) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });
        let solo = pipeline(device, &renderer.weather.fog.resources.layout, &layout, None)?;
        Ok(Self { layout, solo, combined: None, group, uniform, _height: height, height_at: None, job: None })
    }
    fn ensure_combined(&mut self, device: &wgpu::Device, weather: &wgpu::BindGroupLayout,
                       fog: &wgpu::BindGroupLayout) -> Result<(), String> {
        if self.combined.is_none() {
            self.combined = Some(pipeline(device, weather, &self.layout, Some(fog))?);
        }
        Ok(())
    }
}

fn pipeline(device: &wgpu::Device, weather: &wgpu::BindGroupLayout,
            snow: &wgpu::BindGroupLayout, fog: Option<&wgpu::BindGroupLayout>) -> Result<wgpu::ComputePipeline, String> {
    let both = fog.is_some();
    let source = shader_source(both);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(if both { "Ninja snow and fog FVOL froxels" } else { "Ninja passive snowdust froxels" }),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let mut groups: Vec<Option<&wgpu::BindGroupLayout>> = vec![Some(weather)];
    if let Some(fog) = fog { groups.push(Some(fog)); }
    groups.push(Some(snow));
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Ninja passive snowdust froxel pipeline layout"),
        bind_group_layouts: &groups, immediate_size: 0,
    });
    Ok(device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("Ninja passive snowdust froxel integration"),
        layout: Some(&layout), module: &module, entry_point: Some("cs_main"),
        compilation_options: Default::default(), cache: crate::pipeline_cache::get().as_ref(),
    }))
}

fn shader_source(fog: bool) -> String {
    let mut base = include_str!("../froxel_fog.wgsl").to_string();
    let mut sources = String::new();
    if fog {
        base = base.replace("// NINJA_FOG_RAY", "let ninja_ray = ninja_fog_ray_bounds(settings.camera_pos_time.xyz, ray);")
            .replace("// NINJA_FOG_SEGMENT", "let ninja = ninja_fog_segment(ray, ninja_ray, previous_depth, current_depth, sun_direction, camera_forward, world_ray(uv + vec2<f32>(1.0 / f32(FROXEL_X), 0.0)), world_ray(uv + vec2<f32>(0.0, 1.0 / f32(FROXEL_Y))));")
            .replace("/* NINJA_FOG_EXTINCTION */", "+ ninja.optical_depth")
            .replace("// NINJA_FOG_COLOR", "+ ninja.scattering")
            .replace("/* NINJA_FOG_EMISSION */", "+ ninja.emission * ninja_fog_emission_integral(extinction, step_transmittance)");
        sources.push_str(include_str!("ninja_fog_froxel.wgsl"));
        sources.push_str(jka_ninja::fog_volume_wgsl());
        sources.push_str(jka_ninja::fog_mist_wgsl());
    }
    base = base.replace("// NINJA_SNOW_DUST_SEGMENT", "let snowdust = ninja_snow_dust_segment(ray, previous_depth, current_depth);")
        .replace("/* NINJA_SNOW_DUST_EXTINCTION */", "+ snowdust.optical_depth")
        .replace("// NINJA_SNOW_DUST_COLOR", "+ snowdust.scattering")
        .replace("/* NINJA_SNOW_DUST_EMISSION */", "+ snowdust.emission * ninja_snow_dust_emission_integral(extinction, step_transmittance)");
    let snow_code = include_str!("ninja_snow_drift_froxel.wgsl").to_string();
    sources.push_str(&if fog { snow_code.replace("@group(1)", "@group(2)") } else { snow_code });
    sources.push_str(jka_ninja::snow_drift_volume_wgsl());
    base.push_str(&format!("\nfn ninja_snow_dust_emission_integral(tau: f32, transmittance: f32) -> f32 {{\n    if tau < 1.0e-4 {{ return 1.0 - 0.5 * tau; }}\n    return (1.0 - transmittance) / tau;\n}}\n{sources}"));
    base
}

fn build_snow_heights(source: &SnowSource, left: Vec2) -> HeightBuild {
    let step = EXTENT_MAP_UNITS / RES as f32;
    let mut rgba = vec![0f32; RES * RES * 4];
    for row in 0..RES {
        for col in 0..RES {
            let x = left.x + (col as f32 + 0.5) * step;
            let z = left.y + (row as f32 + 0.5) * step;
            let key = ((x / SNOW_SHELL_CHUNK_SIZE).floor() as i32,
                       (z / SNOW_SHELL_CHUNK_SIZE).floor() as i32);
            let Some(tris) = source.cells.get(&key) else { continue; };
            let mut best = f32::NEG_INFINITY;
            for &index in tris {
                let Some(tri) = source.triangles.get(index as usize) else { continue; };
                let [a,b,c] = tri.vertices.map(|v| v.position);
                let edge = |u: [f32;3], v: [f32;3]| (v[0]-u[0], v[2]-u[2]);
                let (bx, bz) = edge(a,b);
                let (cx, cz) = edge(a,c);
                let det = bx * cz - cx * bz;
                if det.abs() <= 1.0e-8 { continue; }
                let px = x-a[0]; let pz = z-a[2];
                let u = (px*cz-cx*pz)/det;
                let v = (bx*pz-px*bz)/det;
                if u < -0.0001 || v < -0.0001 || u+v > 1.0001 { continue; }
                let y = a[1] + u*(b[1]-a[1]) + v*(c[1]-a[1]);
                if y.is_finite() { best = best.max(y); }
            }
            if best.is_finite() {
                let base = 4*(row*RES + col);
                rgba[base] = best;
                rgba[base+3] = 1.0;
            }
        }
    }
    HeightBuild { left, rgba }
}

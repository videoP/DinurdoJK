//! Blue Water's ocean spectrum, FFT and foam field on wgpu.
//!
//! The three WGSL stages below are the preserved upstream files, without
//! shader edits. The host owns scheduling and integration with JKA surfaces.
//! Copyright (c) 2026 Bill Yang. MIT; see vendor/blue-water-sea-and-sky/LICENSE.

// Temporary facade for the upstream clipmap's 52-degree reference lens.
// The original naval_render crate is absent from the published reference tree.
extern crate self as naval_render;
pub mod camera {
    pub const DEFAULT_FOV_Y_DEGREES: f32 = 52.0;
}
#[path = "../../../vendor/blue-water-sea-and-sky/ocean/src/geometry.rs"]
pub mod geometry;
pub mod foam_texture;
pub mod shader_composer;
pub mod original_surface;
pub mod submerged_mesh;
pub mod scene_copy;
pub mod reflection_tiles;
pub mod map_environment;
pub mod underwater;
pub mod caustics;
pub mod rain_heights;
#[path = "../../../vendor/blue-water-sea-and-sky/sky/src/environment.rs"]
pub mod sky_environment;
pub mod surface_waves;
mod crest_limits;
#[path = "../../../vendor/blue-water-sea-and-sky/ocean/src/shore.rs"]
pub mod shore;
#[path = "../../../vendor/blue-water-sea-and-sky/ocean/src/hull_sea.rs"]
pub mod hull_sea;
pub mod lakes;
pub mod godot_water;
use blue_water_core::{sea_state, spectrum, whitecaps, CascadeInfo, QualityTier, WaveParameters};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

const FFT: &str = include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/fft.wgsl");
const RESOLVE: &str =
    include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/resolve.wgsl");
const MIPS: &str = include_str!("../../../vendor/blue-water-sea-and-sky/ocean/shaders/mips.wgsl");
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const BUBBLE_LIFE: f64 = 0.5;
const FOAM_SPREAD: f64 = 0.015;
const SPREAD_MOST: f64 = 1.0;
const ACROSS_SHARE: f64 = 0.25;
const SPREAD_LIMIT: f64 = 0.24;

#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
struct CascadeParams {
    a: [f32; 4],
    b: [f32; 4],
    c: [f32; 4],
    d: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
struct FieldParams {
    time: [f32; 4],
    wind: [f32; 4],
    cascades: [CascadeParams; 3],
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PassParams {
    radix: u32,
    span: u32,
    horizontal: u32,
    evolve: u32,
}

struct FieldSet {
    textures: [wgpu::Texture; 3],
    views: [wgpu::TextureView; 3],
}

pub struct WaveField {
    tier: Vec<CascadeInfo>,
    cascades: Vec<CascadeInfo>,
    n: u32,
    count: u32,
    sea: WaveParameters,
    built_for: Option<(WaveParameters, bool)>,
    breaking: Vec<whitecaps::CascadeBreaking>,
    surface_stats: surface_waves::SpectrumStats,
    spectrum: wgpu::Buffer,
    field_params: wgpu::Buffer,
    _scratch: [wgpu::Buffer; 2],
    sets: [FieldSet; 2],
    transform: wgpu::ComputePipeline,
    resolve: wgpu::ComputePipeline,
    mips: wgpu::ComputePipeline,
    passes: Vec<wgpu::BindGroup>,
    resolves: [wgpu::BindGroup; 2],
    mip_groups: [Vec<wgpu::BindGroup>; 2],
    current: usize,
    last_phase: f32,
    spectrum_fresh: bool,
    crest_limits: crest_limits::CrestLimits,
}

fn uniform(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
fn storage(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}
fn sampled(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2Array,
            multisampled: false,
        },
        count: None,
    }
}
fn output(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: FORMAT,
            view_dimension: wgpu::TextureViewDimension::D2Array,
        },
        count: None,
    }
}
fn level_view(texture: &wgpu::Texture, mip: u32) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        base_mip_level: mip,
        mip_level_count: Some(1),
        ..Default::default()
    })
}
fn pipeline(
    device: &wgpu::Device,
    name: &'static str,
    shader: &str,
    layout: &wgpu::BindGroupLayout,
    entry: &str,
) -> wgpu::ComputePipeline {
    let compilation_started = std::time::Instant::now();
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(name),
        source: wgpu::ShaderSource::Wgsl(shader.into()),
    });
    let pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(name),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(name),
        layout: Some(&pipe_layout),
        module: &module,
        entry_point: Some(entry),
        compilation_options: Default::default(),
        cache: None,
    });
    eprintln!("[BLUE WATER COMPILE] {name}: {:.3} ms (module + pipeline)", compilation_started.elapsed().as_secs_f64() * 1000.0);
    pipeline
}
fn fft_radices(n: u32) -> Vec<u32> {
    assert!(n.is_power_of_two() && n >= 2);
    let bits = n.ilog2();
    let passes = bits.div_ceil(4);
    (0..passes)
        .map(|i| 1 << (bits / passes + u32::from(i < bits % passes)))
        .collect()
}

impl WaveField {
    pub fn new(device: &wgpu::Device, quality: QualityTier) -> Self {
        let tier = quality.cascades();
        let n = tier[0].resolution;
        let count = tier.len() as u32;
        let top = (n / 16).ilog2();
        let cells = u64::from(n) * u64::from(n) * u64::from(count);
        let spectrum = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Blue Water spectrum"),
            size: cells * 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let scratch: [wgpu::Buffer; 2] = std::array::from_fn(|i| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(if i == 0 {
                    "Blue Water FFT A"
                } else {
                    "Blue Water FFT B"
                }),
                size: cells * 32,
                usage: wgpu::BufferUsages::STORAGE,
                mapped_at_creation: false,
            })
        });
        let field_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Blue Water field parameters"),
            size: std::mem::size_of::<FieldParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sets: [FieldSet; 2] = std::array::from_fn(|set| {
            let textures: [wgpu::Texture; 3] = std::array::from_fn(|kind| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(match kind {
                        0 => "Blue Water displacement",
                        1 => "Blue Water derivatives",
                        _ => "Blue Water extras",
                    }),
                    size: wgpu::Extent3d {
                        width: n,
                        height: n,
                        depth_or_array_layers: count,
                    },
                    mip_level_count: top + 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: FORMAT,
                    usage: wgpu::TextureUsages::STORAGE_BINDING
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                })
            });
            let views = std::array::from_fn(|i| {
                textures[i].create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2Array),
                    ..Default::default()
                })
            });
            let _ = set;
            FieldSet { textures, views }
        });
        let transform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Blue Water FFT layout"),
            entries: &[
                uniform(0),
                uniform(1),
                storage(2, true),
                storage(3, true),
                storage(4, false),
            ],
        });
        let resolve_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Blue Water resolve layout"),
            entries: &[
                uniform(0),
                uniform(1),
                storage(2, true),
                sampled(3),
                output(4),
                output(5),
                output(6),
                sampled(7),
            ],
        });
        let mip_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Blue Water mips layout"),
            entries: &[
                sampled(0),
                sampled(1),
                sampled(2),
                output(3),
                output(4),
                output(5),
            ],
        });
        let transform = pipeline(
            device,
            "Blue Water FFT",
            FFT,
            &transform_layout,
            "transform",
        );
        let resolve = pipeline(
            device,
            "Blue Water resolve",
            RESOLVE,
            &resolve_layout,
            "resolve",
        );
        let mips = pipeline(device, "Blue Water mips", MIPS, &mip_layout, "downsample");
        let pass_buffer = |params: PassParams| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Blue Water FFT pass"),
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };
        let radices = fft_radices(n);
        let mut passes = Vec::new();
        let mut make_pass = |index: usize, params: PassParams| {
            let control = pass_buffer(params);
            passes.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Blue Water FFT pass"),
                layout: &transform_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: field_params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: control.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: spectrum.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: scratch[(index + 1) % 2].as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: scratch[index % 2].as_entire_binding(),
                    },
                ],
            }));
        };
        let mut span = 1;
        let mut index = 0;
        for (i, radix) in radices.iter().copied().enumerate() {
            make_pass(
                index,
                PassParams {
                    radix,
                    span,
                    horizontal: 1,
                    evolve: u32::from(i == 0),
                },
            );
            span *= radix;
            index += 1;
        }
        span = 1;
        for radix in radices.iter().copied().take(radices.len() - 1) {
            make_pass(
                index,
                PassParams {
                    radix,
                    span,
                    horizontal: 0,
                    evolve: 0,
                },
            );
            span *= radix;
            index += 1;
        }
        let final_source = &scratch[(index - 1) % 2];
        let final_pass = pass_buffer(PassParams {
            radix: *radices.last().unwrap(),
            span,
            horizontal: 0,
            evolve: 0,
        });
        let resolves = std::array::from_fn(|set| {
            let previous_extras = level_view(&sets[1 - set].textures[2], 0);
            let previous_displacement = level_view(&sets[1 - set].textures[0], 0);
            let outputs: [wgpu::TextureView; 3] =
                std::array::from_fn(|i| level_view(&sets[set].textures[i], 0));
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Blue Water resolve"),
                layout: &resolve_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: field_params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: final_pass.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: final_source.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&previous_extras),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&outputs[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::TextureView(&outputs[1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(&outputs[2]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(&previous_displacement),
                    },
                ],
            })
        });
        let mip_groups = std::array::from_fn(|set| {
            (1..=top)
                .map(|mip| {
                    let source: [wgpu::TextureView; 3] =
                        std::array::from_fn(|i| level_view(&sets[set].textures[i], mip - 1));
                    let output: [wgpu::TextureView; 3] =
                        std::array::from_fn(|i| level_view(&sets[set].textures[i], mip));
                    let entries: Vec<_> = source
                        .iter()
                        .chain(output.iter())
                        .enumerate()
                        .map(|(i, view)| wgpu::BindGroupEntry {
                            binding: i as u32,
                            resource: wgpu::BindingResource::TextureView(view),
                        })
                        .collect();
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("Blue Water mip"),
                        layout: &mip_layout,
                        entries: &entries,
                    })
                })
                .collect()
        });
        let crest_limits = crest_limits::CrestLimits::new(device, &sets, &field_params, n, count);
        Self {
            crest_limits,
            tier: tier.clone(),
            cascades: tier,
            n,
            count,
            sea: WaveParameters::default(),
            built_for: None,
            breaking: Vec::new(),
            surface_stats: surface_waves::SpectrumStats::default(),
            spectrum,
            field_params,
            _scratch: scratch,
            sets,
            transform,
            resolve,
            mips,
            passes,
            resolves,
            mip_groups,
            current: 0,
            last_phase: -1.0,
            spectrum_fresh: false,
        }
    }

    pub fn current_index(&self) -> usize {
        self.current
    }
    pub fn set_crest_limits(&mut self, enabled: bool) {
        if enabled && !self.crest_limits.enabled { self.spectrum_fresh = true; }
        self.crest_limits.enabled = enabled;
    }
    pub fn request_update(&mut self) {
        self.spectrum_fresh = true;
    }
    pub fn crest_limits_buffer(&self) -> &wgpu::Buffer {
        &self.crest_limits.buffer
    }
    pub fn prepare_composite_crest_limits(&mut self, device: &wgpu::Device, waves: &surface_waves::SurfaceWaves) {
        self.crest_limits.prepare_composite(device, waves, &self.field_params);
    }
    pub fn views_for(&self, set: usize) -> &[wgpu::TextureView; 3] {
        &self.sets[set].views
    }
    pub fn resolution(&self) -> u32 {
        self.n
    }
    pub fn cascades(&self) -> &[CascadeInfo] {
        &self.cascades
    }
    pub fn surface_stats(&self) -> &surface_waves::SpectrumStats {
        &self.surface_stats
    }
    pub fn sea(&self) -> WaveParameters {
        self.sea
    }
    pub fn input_parameters(&self) -> WaveParameters {
        self.built_for.map_or(WaveParameters::default(), |(params, _)| params)
    }
    pub fn breaking(&self) -> &[whitecaps::CascadeBreaking] {
        &self.breaking
    }
    pub fn displacement(&self) -> &wgpu::TextureView {
        &self.sets[self.current].views[0]
    }
    pub fn derivatives(&self) -> &wgpu::TextureView {
        &self.sets[self.current].views[1]
    }
    pub fn extras(&self) -> &wgpu::TextureView {
        &self.sets[self.current].views[2]
    }
    pub fn textures(&self) -> &[wgpu::Texture; 3] {
        &self.sets[self.current].textures
    }

    pub fn rebuild_spectrum(
        &mut self,
        queue: &wgpu::Queue,
        params: WaveParameters,
        realistic: bool,
    ) {
        if self.built_for == Some((params, realistic)) {
            return;
        }
        self.sea = sea_state::drawn_sea(&params, realistic);
        // Explicit wavelengths also need room in the FFT tiles. The upstream
        // growth rule does not require its automatic height/wind sea-state policy.
        self.cascades = sea_state::sea_state_cascades(&self.tier, self.sea.peak_wavelength);
        let wave = spectrum::build_spectrum(&self.cascades, &self.sea);
        self.surface_stats = surface_waves::SpectrumStats::from_spectrum(&wave);
        self.surface_stats.set_shore_tables(&wave, self.sea.wind_speed);
        let n = self.n as usize;
        let count = self.count as usize;
        let mut atlas = vec![0f32; n * n * count * 4];
        for (c, cascade) in wave.cascades.iter().enumerate() {
            for z in 0..n {
                let row = &cascade.amplitudes[z * n * 4..(z + 1) * n * 4];
                atlas[(z * count + c) * n * 4..(z * count + c + 1) * n * 4].copy_from_slice(row);
            }
        }
        queue.write_buffer(&self.spectrum, 0, bytemuck::cast_slice(&atlas));
        let (wz, wx) = self.sea.wind_direction.sin_cos();
        self.breaking = whitecaps::cascade_breaking(&wave, wx, wz, self.sea.choppiness.max(0.0));
        self.built_for = Some((params, realistic));
        self.spectrum_fresh = true;
    }

    /// Encodes only Blue Water work. The caller should skip this when another backend is active.
    /// Foam lifetime is expressed in periods, as in the upstream field.
    pub fn update(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        params: WaveParameters,
        realistic: bool,
        time: f64,
        dt: f64,
        foam_coverage: f64,
        foam_lifetime: f64,
    ) -> bool {
        self.rebuild_spectrum(queue, params, realistic);
        if self.spectrum_fresh && self.crest_limits.enabled { self.crest_limits.reset(encoder); }
        let phase = time.rem_euclid(spectrum::FOLD_PERIOD) / spectrum::FOLD_PERIOD;
        if !self.spectrum_fresh && dt <= 0.0 && phase as f32 == self.last_phase {
            return false;
        }
        self.spectrum_fresh = false;
        self.last_phase = phase as f32;
        let (wz, wx) = self.sea.wind_direction.sin_cos();
        whitecaps::set_breaking_thresholds(
            &mut self.breaking,
            whitecaps::whitecap_depth(params.wind_speed, foam_coverage),
        );
        let n = self.n;
        let mut field = FieldParams {
            time: [
                phase as f32,
                self.sea.choppiness as f32,
                dt.max(0.0) as f32,
                (std::f64::consts::TAU * (time / 90.0).fract()) as f32,
            ],
            wind: [wx as f32, wz as f32, n as f32, self.count as f32],
            ..Default::default()
        };
        for (c, breaking) in self.breaking.iter().enumerate() {
            let cascade = self.cascades[c];
            let decay = (foam_lifetime * breaking.period).max(1e-3);
            let texel = cascade.size / f64::from(n);
            let gain = dt.max(0.0) / (texel * texel);
            let period = breaking.period;
            let wavelength = spectrum::GRAVITY * period * period / std::f64::consts::TAU;
            let along = if period > 0.0 {
                SPREAD_MOST.min(FOAM_SPREAD * wavelength * wavelength / period)
            } else {
                0.0
            };
            let across = along * ACROSS_SHARE;
            let (streaks, lines) = if c == 1 {
                let t = ((params.wind_speed - 13.0) / 7.0).clamp(0.0, 1.0);
                let strength = t * t * (3.0 - 2.0 * t);
                if strength > 0.0 {
                    let drift = 0.035 * params.wind_speed / texel;
                    let cycles = cascade.size / 18.0;
                    let mut pair = [(-wz * cycles).round() as f32, (wx * cycles).round() as f32];
                    if pair == [0.0, 0.0] {
                        pair = [0.0, 1.0];
                    }
                    (
                        [
                            (drift * wx) as f32,
                            (drift * wz) as f32,
                            30.0,
                            (0.03 * strength) as f32,
                        ],
                        [
                            pair[0],
                            pair[1],
                            (0.035 * params.wind_speed / texel) as f32,
                            0.8,
                        ],
                    )
                } else {
                    ([0.0; 4], [0.0; 4])
                }
            } else {
                ([0.0; 4], [0.0; 4])
            };
            field.cascades[c] = CascadeParams {
                a: [
                    (std::f64::consts::TAU / cascade.size) as f32,
                    decay as f32,
                    breaking.threshold.min(1e4) as f32,
                    (decay * BUBBLE_LIFE) as f32,
                ],
                b: [
                    breaking.compression as f32,
                    breaking.face as f32,
                    SPREAD_LIMIT.min(gain * (along * wx * wx + across * wz * wz)) as f32,
                    SPREAD_LIMIT.min(gain * (along * wz * wz + across * wx * wx)) as f32,
                ],
                c: streaks,
                d: lines,
            };
        }
        queue.write_buffer(&self.field_params, 0, bytemuck::bytes_of(&field));
        let next = 1 - self.current;
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Blue Water waves"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.transform);
        for group in &self.passes {
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups((n * self.count).div_ceil(16), n.div_ceil(16), 1);
        }
        pass.set_pipeline(&self.resolve);
        pass.set_bind_group(0, &self.resolves[next], &[]);
        pass.dispatch_workgroups(n.div_ceil(8), n.div_ceil(8), self.count);
        pass.set_pipeline(&self.mips);
        for (mip, group) in self.mip_groups[next].iter().enumerate() {
            let size = n >> (mip + 1);
            pass.set_bind_group(0, group, &[]);
            pass.dispatch_workgroups(size.div_ceil(8), size.div_ceil(8), self.count);
        }
        drop(pass);
        self.crest_limits.encode(encoder, next, n, self.count);
        self.current = next;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_fft_and_foam_dispatch_validate_on_gpu() {
        futures_lite::future::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .expect("GPU adapter for Blue Water validation");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_limits: adapter.limits(),
                    ..Default::default()
                })
                .await
                .expect("GPU device");
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let mut field = WaveField::new(&device, QualityTier::High);
            let mut encoder = device.create_command_encoder(&Default::default());
            assert!(field.update(
                &queue,
                &mut encoder,
                WaveParameters::default(),
                true,
                1.0 / 48.0,
                1.0 / 48.0,
                1.0,
                1.0
            ));
            let surface = surface_waves::SurfaceWaves::new(&device, &field);
            surface.write(&queue, &field, 1.0 / 48.0, 1.0, 1.0);
            let _surface_group = surface.group(&field);
            let uniforms = surface_waves::WaveUniforms::from_field(
                &field, 1.0 / 48.0, 1.0, 1.0,
            );
            assert_eq!(uniforms.info[1], 3.0);
            assert_eq!(uniforms.info[2], 256.0);
            assert!(uniforms.tile[..3].iter().all(|size| *size > 0.0));
            let n = field.resolution();
            let read = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Blue Water displacement readback"),
                size: u64::from(n) * u64::from(n) * 8,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &field.textures()[0],
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &read,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(n * 8),
                        rows_per_image: Some(n),
                    },
                },
                wgpu::Extent3d {
                    width: n,
                    height: n,
                    depth_or_array_layers: 1,
                },
            );
            queue.submit(Some(encoder.finish()));
            read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .expect("Blue Water GPU submission");
            assert!(
                scope.pop().await.is_none(),
                "Blue Water GPU validation error"
            );
            let bytes = read
                .slice(..)
                .get_mapped_range()
                .expect("Blue Water displacement map");
            assert!(
                bytes.iter().any(|&byte| byte != 0),
                "Blue Water FFT produced empty displacement"
            );
        });
    }
}

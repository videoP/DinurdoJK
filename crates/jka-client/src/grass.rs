//! Faithful wgpu port of 2Retr0/GodotGrass for JKA `surfaceSprites vertical` surfaces.
//!
//! The Godot project remains the visual/algorithmic authority: blade meshes, material
//! defaults, FastNoiseLite inputs, shader math, tile size, LOD distances and LOD density
//! parameters are retained. The unavoidable adaptation is placement: JKA BSP triangles
//! provide the blade roots instead of GodotGrass' procedural heightmap tiles.
//!
//! GodotGrass is MIT licensed:
//! Copyright (c) 2024 Ethan Truong
//! Permission is hereby granted, free of charge, to any person obtaining a copy of this
//! software and associated documentation files (the "Software"), to deal in the Software
//! without restriction, including without limitation the rights to use, copy, modify,
//! merge, publish, distribute, sublicense, and/or sell copies of the Software, and to
//! permit persons to whom the Software is furnished to do so, subject to the following
//! conditions: the above copyright notice and this permission notice shall be included
//! in all copies or substantial portions of the Software. THE SOFTWARE IS PROVIDED "AS IS",
//! WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE
//! WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.

use crate::scene::{DirectionalSun, GrassInstance, GrassPatch};
use bytemuck::{Pod, Zeroable};
use fastnoise_lite::{
    CellularDistanceFunction, CellularReturnType, DomainWarpType, FastNoiseLite, FractalType,
    NoiseType,
};
use glam::{Mat4, Vec3, Vec4};
use std::{ops::Range, time::Instant};
use wgpu::util::DeviceExt;

/// The GodotGrass demo is authored in meters. Treat 64 JKA units as one Godot meter.
const GODOT_WORLD_SCALE: f32 = 1.0 / 64.0;
const SOURCE_REFERENCE_SPRITE_HEIGHT: f32 = 36.0;
const SOURCE_CLUMPING_FACTOR: f32 = 0.5;
const GRASS_CLOUD_WIND_INHERITANCE: f32 = 0.20;
const DEFAULT_CLOUD_WIND_SPEED: f32 = 120.0;
const DEFAULT_CLOUD_WIND_DIRECTION: f32 = 20.0;
const SOURCE_MAP_RADIUS_METERS: f32 = 200.0;
const SOURCE_LOD0_METERS: f32 = 12.0;
const SOURCE_LOD1_METERS: f32 = 40.0;
const SOURCE_LOD2_METERS: f32 = 70.0;
const SOURCE_LOD3_METERS: f32 = 100.0;

// main.gd passes these to create_grass_multimesh(). That function makes row_size
// proportional to the density parameter and instance_count=row_size^2, so the actual
// instance ratios are density^2. Preserve that exact behavior.
const SOURCE_LOD_DENSITIES: [f32; 5] = [1.0, 0.5, 0.25, 0.1, 0.02];
const CLUMP_NOISE_SIZE: u32 = 256;
// NoiseTexture2D defaults to 512x512 when width/height are not specified.
const WIND_NOISE_SIZE: u32 = 512;
const BLADE_DETAIL_SIZE: u32 = 64;
const BLADE_DETAIL_RGBA: &[u8; (BLADE_DETAIL_SIZE * BLADE_DETAIL_SIZE * 4) as usize] =
    include_bytes!("grass_detail.rgba");
const GODOT_SEAMLESS_SKIRT: f32 = 0.1;
const PREPARED_WORDS_PER_BLADE: u64 = 11;
const PREPARED_BLADE_BYTES: u64 = PREPARED_WORDS_PER_BLADE * 4;
const PREPARED_VISIBLE_CAPACITY: u32 = 524_288;
const PREPARE_WORKGROUP_SIZE: u32 = 64;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BladeVertex {
    position: [f32; 3],
    uv: [f32; 2],
}

const BLADE_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2];

type GrassInstanceGpu = GrassInstance;

const VISIBLE_INDEX_ATTRIBUTES: [wgpu::VertexAttribute; 1] =
    wgpu::vertex_attr_array![2 => Uint32];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GrassGlobals {
    /// xyz points from the surface toward the sun; w is normalized q3map strength.
    sun_direction_strength: [f32; 4],
    sun_color: [f32; 4],
    base_color: [f32; 4],
    tip_color: [f32; 4],
    sss_color: [f32; 4],
    /// x=JKA->Godot meters, y=clumping_factor, z=cloud/world direction radians, w=reference JKA height.
    params: [f32; 4],
    /// xy=unit prevailing X/Z direction, z=full atmospheric m/s, w=grass response m/s.
    weather_wind: [f32; 4],
}

struct GrassPatchGpu {
    chunk: u32,
    instances: Range<u32>,
    pvs_signature: Vec<u64>,
    center: Vec3,
    radius: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GrassPrepareJobGpu {
    low_visible_offset: u32,
    low_count: u32,
    low_output_offset: u32,
    mid_visible_offset: u32,
    mid_count: u32,
    mid_output_offset: u32,
    high_visible_offset: u32,
    high_count: u32,
    high_output_offset: u32,
    _pad: [u32; 3],
    camera_pos_time: [f32; 4],
    player_position_enabled: [f32; 4],
}

struct GrassInstanceChunkGpu {
    // Keep the source blade buffer alive for both the fallback vertex path and
    // the optimized compute-prepared path.
    _buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    prepare_job_buffer: wgpu::Buffer,
    prepare_bind_group: wgpu::BindGroup,
}

pub struct GrassMapGpu {
    // Canonical 24-byte blade records are split only when the device's maximum
    // storage-buffer binding size requires it.
    instance_chunks: Vec<GrassInstanceChunkGpu>,
    visible_index_buffer: wgpu::Buffer,
    visible_index_capacity: u32,
    _prepared_buffer: wgpu::Buffer,
    prepared_bind_group: wgpu::BindGroup,
    prepared_capacity: u32,
    patches: Vec<GrassPatchGpu>,
    blade_count: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct GrassDrawStats {
    pub cpu_ms: f64,
    pub patches_total: u32,
    pub patches_visible: u32,
    pub pvs_rejected: u32,
    pub frustum_rejected: u32,
    pub distance_rejected: u32,
    pub lod_empty: u32,
    /// Number of actual indexed draw records consumed by the GPU.
    pub draw_calls: u32,
    /// Number of RenderPass draw API calls. With multi-draw this is normally <= 2.
    pub submit_calls: u32,
    pub high_blades: u64,
    pub mid_blades: u64,
    pub low_blades: u64,
    pub triangles: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct GrassPreparedChunk {
    low_offset: u32,
    low_count: u32,
    low_output_offset: u32,
    mid_offset: u32,
    mid_count: u32,
    mid_output_offset: u32,
    high_offset: u32,
    high_count: u32,
    high_output_offset: u32,
}

pub struct GrassPreparedDraw {
    stats: GrassDrawStats,
    chunks: Vec<GrassPreparedChunk>,
    total_visible: u32,
    low_total: u32,
    mid_total: u32,
    high_total: u32,
    gpu_precompute: bool,
    front_to_back: bool,
    // Normally None: a 1M-index persistent buffer covers ordinary scenes. If a
    // pathological view exceeds that, allocate one exact temporary buffer and use
    // the faithful fallback vertex path for that frame.
    overflow_index_buffer: Option<wgpu::Buffer>,
}

pub struct GrassRenderer {
    pipeline_layout: wgpu::PipelineLayout,
    map_layout: wgpu::BindGroupLayout,
    shader: wgpu::ShaderModule,
    pipeline: wgpu::RenderPipeline,
    prepared_pipeline_layout: wgpu::PipelineLayout,
    prepared_layout: wgpu::BindGroupLayout,
    prepared_shader: wgpu::ShaderModule,
    prepared_pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    bevy_shadow_pipeline: wgpu::RenderPipeline,
    prepare_layout: wgpu::BindGroupLayout,
    prepare_pipeline: wgpu::ComputePipeline,
    high_vertex_buffer: wgpu::Buffer,
    high_index_buffer: wgpu::Buffer,
    high_index_count: u32,
    mid_vertex_buffer: wgpu::Buffer,
    mid_index_buffer: wgpu::Buffer,
    mid_index_count: u32,
    low_vertex_buffer: wgpu::Buffer,
    low_index_buffer: wgpu::Buffer,
    low_index_count: u32,
    _clump_noise: wgpu::Texture,
    _wind_noise: wgpu::Texture,
    _blade_detail: wgpu::Texture,
    _noise_sampler: wgpu::Sampler,
    globals_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl GrassRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        camera_layout: &wgpu::BindGroupLayout,
        shadow_receiver_layout: &wgpu::BindGroupLayout,
        shadow_caster_layout: &wgpu::BindGroupLayout,
        _multi_draw_supported: bool,
        surface_format: wgpu::TextureFormat,
        msaa_samples: u32,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("GodotGrass bind group layout"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture_entry(4),
            ],
        });
        let map_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("GodotGrass compact instance layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("GodotGrass pipeline layout"),
            bind_group_layouts: &[
                Some(camera_layout),
                Some(&layout),
                Some(shadow_receiver_layout),
                Some(&map_layout),
            ],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("GodotGrass faithful fallback WGSL"),
            source: wgpu::ShaderSource::Wgsl(include_str!("grass.wgsl").into()),
        });

        let prepared_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("GodotGrass prepared blade layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let prepared_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("GodotGrass prepared pipeline layout"),
                bind_group_layouts: &[
                    Some(camera_layout),
                    Some(&layout),
                    Some(shadow_receiver_layout),
                    Some(&prepared_layout),
                ],
                immediate_size: 0,
            });
        let prepared_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("GodotGrass prepared WGSL"),
            source: wgpu::ShaderSource::Wgsl(include_str!("grass_prepared.wgsl").into()),
        });

        let prepare_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("GodotGrass compute preparation layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let prepare_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("GodotGrass compute preparation pipeline layout"),
                bind_group_layouts: &[
                    Some(&layout),
                    Some(&prepare_layout),
                    Some(shadow_receiver_layout),
                ],
                immediate_size: 0,
            });
        let prepare_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("GodotGrass compute preparation WGSL"),
            source: wgpu::ShaderSource::Wgsl(include_str!("grass_prepare.wgsl").into()),
        });
        let prepare_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("GodotGrass per-visible-blade preparation"),
            layout: Some(&prepare_pipeline_layout),
            module: &prepare_shader,
            entry_point: Some("cs_main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let (high_vertices, high_indices) = high_blade_mesh();
        let high_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("GodotGrass high blade vertices"),
            contents: bytemuck::cast_slice(&high_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let high_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("GodotGrass high blade indices"),
            contents: bytemuck::cast_slice(&high_indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let (mid_vertices, mid_indices) = mid_blade_mesh();
        let mid_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("GodotGrass middle blade vertices"),
            contents: bytemuck::cast_slice(&mid_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let mid_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("GodotGrass middle blade indices"),
            contents: bytemuck::cast_slice(&mid_indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let (low_vertices, low_indices) = low_blade_mesh();
        let low_vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("GodotGrass low blade vertices"),
            contents: bytemuck::cast_slice(&low_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let low_index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("GodotGrass low blade indices"),
            contents: bytemuck::cast_slice(&low_indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        // Faithful copies of mat_grass.tres noise resources. Godot normalizes and makes
        // NoiseTexture2D seamless before the shader sees it; reproduce that CPU-side.
        let clump_data = godot_clump_noise();
        let wind_data = godot_wind_noise();
        let clump_noise = create_noise_texture(
            device,
            queue,
            "GodotGrass clump noise",
            CLUMP_NOISE_SIZE,
            CLUMP_NOISE_SIZE,
            &clump_data,
        );
        let wind_noise = create_noise_texture(
            device,
            queue,
            "GodotGrass wind noise",
            WIND_NOISE_SIZE,
            WIND_NOISE_SIZE,
            &wind_data,
        );
        // Final blade pigmentation is baked once into a tiny 64x64 RGBA texture.
        // One cached lookup supplies stronger fibre/mottle/chlorophyll variation
        // without bringing back per-fragment procedural noise.
        let blade_detail = create_rgba_texture(
            device,
            queue,
            "JKA grass baked blade detail",
            BLADE_DETAIL_SIZE,
            BLADE_DETAIL_SIZE,
            BLADE_DETAIL_RGBA,
        );
        let clump_view = clump_noise.create_view(&Default::default());
        let wind_view = wind_noise.create_view(&Default::default());
        let blade_detail_view = blade_detail.create_view(&Default::default());
        let noise_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("GodotGrass repeat noise sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        let globals = globals_for_environment(
            None,
            DEFAULT_CLOUD_WIND_SPEED,
            DEFAULT_CLOUD_WIND_DIRECTION,
            true,
        );
        let globals_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("GodotGrass globals"),
            contents: bytemuck::bytes_of(&globals),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("GodotGrass bind group"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&clump_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&wind_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&noise_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: globals_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&blade_detail_view),
                },
            ],
        });
        let pipeline = create_pipeline(
            device,
            &pipeline_layout,
            &shader,
            surface_format,
            msaa_samples,
        );
        let prepared_pipeline = create_prepared_pipeline(
            device,
            &prepared_pipeline_layout,
            &prepared_shader,
            surface_format,
            msaa_samples,
        );
        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("GodotGrass near shadow WGSL"),
            source: wgpu::ShaderSource::Wgsl(include_str!("grass_shadow.wgsl").into()),
        });
        let shadow_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("GodotGrass near shadow pipeline layout"),
                bind_group_layouts: &[Some(shadow_caster_layout), Some(&prepared_layout)],
                immediate_size: 0,
            });
        let shadow_pipeline = create_grass_shadow_pipeline(
            device,
            &shadow_pipeline_layout,
            &shadow_shader,
            false,
        );
        let bevy_shadow_pipeline = create_grass_shadow_pipeline(
            device,
            &shadow_pipeline_layout,
            &shadow_shader,
            true,
        );
        println!(
            "Grass GPU path: A/B compute preparation + high/mid/low true instanced LOD (faithful fallback retained)"
        );

        Self {
            pipeline_layout,
            map_layout,
            shader,
            pipeline,
            prepared_pipeline_layout,
            prepared_layout,
            prepared_shader,
            prepared_pipeline,
            shadow_pipeline,
            bevy_shadow_pipeline,
            prepare_layout,
            prepare_pipeline,
            high_vertex_buffer,
            high_index_buffer,
            high_index_count: high_indices.len() as u32,
            mid_vertex_buffer,
            mid_index_buffer,
            mid_index_count: mid_indices.len() as u32,
            low_vertex_buffer,
            low_index_buffer,
            low_index_count: low_indices.len() as u32,
            _clump_noise: clump_noise,
            _wind_noise: wind_noise,
            _blade_detail: blade_detail,
            _noise_sampler: noise_sampler,
            globals_buffer,
            bind_group,
        }
    }

    /// Canonical GodotGrass wind-noise texture. Weather effects reuse this exact
    /// resource so rain and grass share one noise source instead of generating a
    /// second weather-noise texture.
    pub(crate) fn wind_noise_texture(&self) -> &wgpu::Texture {
        &self._wind_noise
    }

    pub fn rebuild_pipeline(
        &mut self,
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
        msaa_samples: u32,
    ) {
        self.pipeline = create_pipeline(
            device,
            &self.pipeline_layout,
            &self.shader,
            surface_format,
            msaa_samples,
        );
        self.prepared_pipeline = create_prepared_pipeline(
            device,
            &self.prepared_pipeline_layout,
            &self.prepared_shader,
            surface_format,
            msaa_samples,
        );
    }

    pub fn update_environment(
        &self,
        queue: &wgpu::Queue,
        sun: DirectionalSun,
        cloud_wind_speed: f32,
        cloud_wind_direction: f32,
        realtime_sun_lighting: bool,
    ) {
        queue.write_buffer(
            &self.globals_buffer,
            0,
            bytemuck::bytes_of(&globals_for_environment(
                Some(sun),
                cloud_wind_speed,
                cloud_wind_direction,
                realtime_sun_lighting,
            )),
        );
    }

    pub fn upload_map(&self, device: &wgpu::Device, patches: &[GrassPatch]) -> Option<GrassMapGpu> {
        let blade_count: usize = patches.iter().map(|patch| patch.instances.len()).sum();
        if blade_count == 0 {
            return None;
        }

        let instance_stride = std::mem::size_of::<GrassInstanceGpu>();
        let max_binding_bytes = device.limits().max_storage_buffer_binding_size as usize;
        let max_instances_per_chunk = (max_binding_bytes / instance_stride).max(1);
        let chunk_capacity = max_instances_per_chunk.min(blade_count);
        let mut cpu_chunks: Vec<Vec<GrassInstanceGpu>> =
            vec![Vec::with_capacity(chunk_capacity)];
        let mut gpu_patches = Vec::with_capacity(patches.len());

        for patch in patches {
            // Map preparation already seed-sorts each patch and pre-packs the static
            // clump samples into the two alpha bytes. Renderer upload therefore only
            // copies compact 24-byte records; no per-blade sorting/noise work remains.
            let ordered = &patch.instances;

            // A normal 256-unit grass patch is tiny compared with the guaranteed
            // storage-binding limit. If an authored pathological patch ever exceeds
            // it, split that patch into several identical visibility records.
            let mut cursor = 0usize;
            while cursor < ordered.len() {
                let current = cpu_chunks.last_mut().expect("grass chunk exists");
                if current.len() >= max_instances_per_chunk {
                    cpu_chunks.push(Vec::with_capacity(chunk_capacity));
                    continue;
                }
                let room = max_instances_per_chunk - current.len();
                let take = room.min(ordered.len() - cursor);
                let start = current.len() as u32;
                current.extend_from_slice(&ordered[cursor..cursor + take]);
                let end = current.len() as u32;
                gpu_patches.push(GrassPatchGpu {
                    chunk: (cpu_chunks.len() - 1) as u32,
                    instances: start..end,
                    pvs_signature: patch.pvs_signature.clone(),
                    center: Vec3::from_array(patch.center),
                    radius: patch.radius,
                });
                cursor += take;
                if cursor < ordered.len() {
                    cpu_chunks.push(Vec::with_capacity(chunk_capacity));
                }
            }
        }
        cpu_chunks.retain(|chunk| !chunk.is_empty());

        let gpu_bytes = blade_count * instance_stride;
        println!(
            "Grass GPU instances: {} blade(s), {:.1} MiB at {} bytes/blade across {} storage chunk(s) (max binding {:.1} MiB)",
            blade_count,
            gpu_bytes as f64 / (1024.0 * 1024.0),
            instance_stride,
            cpu_chunks.len(),
            max_binding_bytes as f64 / (1024.0 * 1024.0),
        );

        // Keep the compact visible index list small and transient. The optimized
        // compute path prepares at most 524k visible blades (~22 MiB at 44 bytes each),
        // which is far above the ~90k visible blades in the current stress view.
        // Frames above that limit automatically use the faithful fallback vertex path.
        let visible_index_capacity = (blade_count as u32).min(1_048_576).max(1);
        let visible_index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA GodotGrass compact visible blade indices"),
            size: u64::from(visible_index_capacity) * std::mem::size_of::<u32>() as u64,
            usage: wgpu::BufferUsages::VERTEX
                | wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let prepared_capacity = (blade_count as u32)
            .min(PREPARED_VISIBLE_CAPACITY)
            .max(1);
        let prepared_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("JKA GodotGrass GPU-prepared visible blades"),
            size: u64::from(prepared_capacity) * PREPARED_BLADE_BYTES,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let prepared_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("JKA GodotGrass prepared render bind group"),
            layout: &self.prepared_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: prepared_buffer.as_entire_binding(),
            }],
        });

        let mut instance_chunks = Vec::with_capacity(cpu_chunks.len());
        for instances in cpu_chunks {
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("JKA GodotGrass instance storage chunk"),
                contents: bytemuck::cast_slice(&instances),
                usage: wgpu::BufferUsages::STORAGE,
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("JKA GodotGrass faithful fallback instance bind group"),
                layout: &self.map_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                }],
            });
            let prepare_job_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("JKA GodotGrass prepare job uniform"),
                size: std::mem::size_of::<GrassPrepareJobGpu>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let prepare_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("JKA GodotGrass per-chunk compute prepare bind group"),
                layout: &self.prepare_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: visible_index_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: prepared_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: prepare_job_buffer.as_entire_binding(),
                    },
                ],
            });
            instance_chunks.push(GrassInstanceChunkGpu {
                _buffer: buffer,
                bind_group,
                prepare_job_buffer,
                prepare_bind_group,
            });
        }

        println!(
            "Grass visible buffers: index={:.1} MiB capacity={} blade(s), prepared={:.1} MiB capacity={} blade(s)",
            u64::from(visible_index_capacity) as f64 * 4.0 / (1024.0 * 1024.0),
            visible_index_capacity,
            u64::from(prepared_capacity) as f64 * PREPARED_BLADE_BYTES as f64
                / (1024.0 * 1024.0),
            prepared_capacity,
        );

        Some(GrassMapGpu {
            instance_chunks,
            visible_index_buffer,
            visible_index_capacity,
            _prepared_buffer: prepared_buffer,
            prepared_bind_group,
            prepared_capacity,
            patches: gpu_patches,
            blade_count: blade_count as u32,
        })
    }

    /// Select visible grass into physically compact u32 instance-index lists.
    /// Patch/PVS/frustum/distance/LOD decisions are unchanged. The final render uses
    /// one low + one high draw per required storage chunk; on GPUs whose storage
    /// binding can hold the whole map this is literally two indexed instanced draws.
    pub fn prepare_draw(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        map: &GrassMapGpu,
        camera_position: Vec3,
        player_position: Option<Vec3>,
        view_proj: Mat4,
        pvs_cluster: Option<usize>,
        frame_time: f32,
        enable_precompute: bool,
        enable_mid_lod: bool,
        enable_front_to_back: bool,
    ) -> GrassPreparedDraw {
        let started = Instant::now();
        let mut stats = GrassDrawStats {
            patches_total: u32::try_from(map.patches.len()).unwrap_or(u32::MAX),
            ..Default::default()
        };
        let chunk_count = map.instance_chunks.len();
        let mut low_by_chunk = (0..chunk_count)
            .map(|_| Vec::<u32>::with_capacity(16_384))
            .collect::<Vec<_>>();
        let mut mid_by_chunk = (0..chunk_count)
            .map(|_| Vec::<u32>::with_capacity(24_576))
            .collect::<Vec<_>>();
        let mut high_by_chunk = (0..chunk_count)
            .map(|_| Vec::<u32>::with_capacity(32_768))
            .collect::<Vec<_>>();

        // Collect visibility/LOD decisions first. Front-to-back ordering does not need
        // an exact comparison sort: opaque early-Z gets nearly the same benefit from
        // coarse near-to-far ordering. We therefore bucket the ~visible patch set into
        // 64 fixed distance bands across the 200 m grass radius, reducing the ordering
        // work from O(N log N) to O(N) while preserving the exact visible population.
        const FRONT_TO_BACK_BUCKETS: usize = 64;
        let mut visible_patches = Vec::<(usize, f32, u32)>::with_capacity(2_048);
        if map.blade_count != 0 {
            for (patch_index, patch) in map.patches.iter().enumerate() {
                if let Some(cluster) = pvs_cluster {
                    let word = cluster / 64;
                    let bit = cluster % 64;
                    if word >= patch.pvs_signature.len()
                        || patch.pvs_signature[word] & (1_u64 << bit) == 0
                    {
                        stats.pvs_rejected = stats.pvs_rejected.saturating_add(1);
                        continue;
                    }
                }

                if !sphere_intersects_clip_frustum(patch.center, patch.radius, view_proj) {
                    stats.frustum_rejected = stats.frustum_rejected.saturating_add(1);
                    continue;
                }

                let center_distance_world = camera_position.distance(patch.center);
                let nearest_distance_m =
                    ((center_distance_world - patch.radius).max(0.0)) * GODOT_WORLD_SCALE;
                if nearest_distance_m > SOURCE_MAP_RADIUS_METERS {
                    stats.distance_rejected = stats.distance_rejected.saturating_add(1);
                    continue;
                }

                let center_distance_m = center_distance_world * GODOT_WORLD_SCALE;
                let total = patch.instances.end - patch.instances.start;
                let count_fraction = source_lod_count_fraction(center_distance_m);
                let visible_count = ((total as f32) * count_fraction).round() as u32;
                let visible_count = visible_count.min(total);
                if visible_count == 0 {
                    stats.lod_empty = stats.lod_empty.saturating_add(1);
                    continue;
                }
                stats.patches_visible = stats.patches_visible.saturating_add(1);
                visible_patches.push((patch_index, center_distance_m, visible_count));
            }
        }

        if enable_front_to_back && visible_patches.len() > 1 {
            let mut bucket_counts = [0_usize; FRONT_TO_BACK_BUCKETS];
            for &(_, center_distance_m, _) in &visible_patches {
                let normalized =
                    (center_distance_m / SOURCE_MAP_RADIUS_METERS).clamp(0.0, 0.999_999_94);
                let bucket =
                    (normalized * FRONT_TO_BACK_BUCKETS as f32) as usize;
                bucket_counts[bucket.min(FRONT_TO_BACK_BUCKETS - 1)] += 1;
            }

            let mut bucket_offsets = [0_usize; FRONT_TO_BACK_BUCKETS];
            let mut running = 0_usize;
            for bucket in 0..FRONT_TO_BACK_BUCKETS {
                bucket_offsets[bucket] = running;
                running += bucket_counts[bucket];
            }
            let mut write_offsets = bucket_offsets;
            let mut bucketed = vec![(0_usize, 0.0_f32, 0_u32); visible_patches.len()];
            for entry in visible_patches.drain(..) {
                let normalized =
                    (entry.1 / SOURCE_MAP_RADIUS_METERS).clamp(0.0, 0.999_999_94);
                let bucket =
                    ((normalized * FRONT_TO_BACK_BUCKETS as f32) as usize)
                        .min(FRONT_TO_BACK_BUCKETS - 1);
                let destination = write_offsets[bucket];
                bucketed[destination] = entry;
                write_offsets[bucket] += 1;
            }
            visible_patches = bucketed;
        }

        for (patch_index, center_distance_m, visible_count) in visible_patches {
            let patch = &map.patches[patch_index];
            let (_high_weight, mid_weight, low_weight) =
                source_mesh_weights(center_distance_m, enable_mid_lod);
            let low_count = ((visible_count as f32) * low_weight).round() as u32;
            let low_count = low_count.min(visible_count);
            let remaining_after_low = visible_count - low_count;
            let mid_count = ((visible_count as f32) * mid_weight).round() as u32;
            let mid_count = mid_count.min(remaining_after_low);
            // Give any rounding remainder to the closest/highest-detail bucket.
            let high_count = visible_count - low_count - mid_count;
            let chunk = patch.chunk as usize;

            if low_count != 0 {
                low_by_chunk[chunk]
                    .extend(patch.instances.start..patch.instances.start + low_count);
                stats.low_blades += u64::from(low_count);
                stats.triangles += u64::from(low_count) * u64::from(self.low_index_count / 3);
                stats.draw_calls = stats.draw_calls.saturating_add(1);
            }

            if mid_count != 0 {
                let mid_start = patch.instances.start + low_count;
                mid_by_chunk[chunk].extend(mid_start..mid_start + mid_count);
                stats.mid_blades += u64::from(mid_count);
                stats.triangles += u64::from(mid_count) * u64::from(self.mid_index_count / 3);
                stats.draw_calls = stats.draw_calls.saturating_add(1);
            }

            if high_count != 0 {
                let high_start = patch.instances.start + low_count + mid_count;
                high_by_chunk[chunk].extend(high_start..high_start + high_count);
                stats.high_blades += u64::from(high_count);
                stats.triangles +=
                    u64::from(high_count) * u64::from(self.high_index_count / 3);
                stats.draw_calls = stats.draw_calls.saturating_add(1);
            }
        }

        let low_total = u32::try_from(stats.low_blades).unwrap_or(u32::MAX);
        let mid_total = u32::try_from(stats.mid_blades).unwrap_or(u32::MAX);
        let high_total = u32::try_from(stats.high_blades).unwrap_or(u32::MAX);
        let mut compact_indices = Vec::<u32>::with_capacity(
            usize::try_from(stats.low_blades + stats.mid_blades + stats.high_blades).unwrap_or(0),
        );
        let mut prepared_chunks = Vec::with_capacity(chunk_count);
        let mut low_output_cursor = 0_u32;
        let mut mid_output_cursor = low_total;
        let mut high_output_cursor = low_total.saturating_add(mid_total);
        for chunk in 0..chunk_count {
            let low_offset = compact_indices.len() as u32;
            compact_indices.extend_from_slice(&low_by_chunk[chunk]);
            let low_count = compact_indices.len() as u32 - low_offset;
            let mid_offset = compact_indices.len() as u32;
            compact_indices.extend_from_slice(&mid_by_chunk[chunk]);
            let mid_count = compact_indices.len() as u32 - mid_offset;
            let high_offset = compact_indices.len() as u32;
            compact_indices.extend_from_slice(&high_by_chunk[chunk]);
            let high_count = compact_indices.len() as u32 - high_offset;
            let low_output_offset = low_output_cursor;
            let mid_output_offset = mid_output_cursor;
            let high_output_offset = high_output_cursor;
            low_output_cursor = low_output_cursor.saturating_add(low_count);
            mid_output_cursor = mid_output_cursor.saturating_add(mid_count);
            high_output_cursor = high_output_cursor.saturating_add(high_count);
            prepared_chunks.push(GrassPreparedChunk {
                low_offset,
                low_count,
                low_output_offset,
                mid_offset,
                mid_count,
                mid_output_offset,
                high_offset,
                high_count,
                high_output_offset,
            });
        }

        let total_visible = u32::try_from(compact_indices.len()).unwrap_or(u32::MAX);
        let overflow_index_buffer = if total_visible > map.visible_index_capacity {
            Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("JKA GodotGrass overflow compact visible indices"),
                contents: bytemuck::cast_slice(&compact_indices),
                usage: wgpu::BufferUsages::VERTEX,
            }))
        } else {
            if !compact_indices.is_empty() {
                queue.write_buffer(
                    &map.visible_index_buffer,
                    0,
                    bytemuck::cast_slice(&compact_indices),
                );
            }
            None
        };

        let gpu_precompute = enable_precompute
            && overflow_index_buffer.is_none()
            && total_visible <= map.prepared_capacity
            && total_visible <= map.visible_index_capacity;
        if gpu_precompute {
            for (chunk_index, chunk) in prepared_chunks.iter().enumerate() {
                if chunk.low_count == 0 && chunk.mid_count == 0 && chunk.high_count == 0 {
                    continue;
                }
                let job = GrassPrepareJobGpu {
                    low_visible_offset: chunk.low_offset,
                    low_count: chunk.low_count,
                    low_output_offset: chunk.low_output_offset,
                    mid_visible_offset: chunk.mid_offset,
                    mid_count: chunk.mid_count,
                    mid_output_offset: chunk.mid_output_offset,
                    high_visible_offset: chunk.high_offset,
                    high_count: chunk.high_count,
                    high_output_offset: chunk.high_output_offset,
                    _pad: [0; 3],
                    camera_pos_time: [
                        camera_position.x,
                        camera_position.y,
                        camera_position.z,
                        frame_time,
                    ],
                    player_position_enabled: player_position.map_or(
                        [0.0, 0.0, 0.0, 0.0],
                        |position| [position.x, position.y, position.z, 1.0],
                    ),
                };
                queue.write_buffer(
                    &map.instance_chunks[chunk_index].prepare_job_buffer,
                    0,
                    bytemuck::bytes_of(&job),
                );
            }
            // Prepared blades are globally compacted as LOW, MID, HIGH so the
            // render pass consumes at most three true instanced draws.
            stats.submit_calls = (low_total != 0) as u32
                + (mid_total != 0) as u32
                + (high_total != 0) as u32;
        } else {
            // Precompute A/B-off (or pathological visibility) path: retain the
            // faithful per-vertex shader with up to three draws per storage chunk.
            stats.submit_calls = prepared_chunks.iter().fold(0_u32, |count, chunk| {
                count
                    .saturating_add((chunk.low_count != 0) as u32)
                    .saturating_add((chunk.mid_count != 0) as u32)
                    .saturating_add((chunk.high_count != 0) as u32)
            });
        }

        stats.cpu_ms = started.elapsed().as_secs_f64() * 1000.0;
        GrassPreparedDraw {
            stats,
            chunks: prepared_chunks,
            total_visible,
            low_total,
            mid_total,
            high_total,
            gpu_precompute,
            front_to_back: enable_front_to_back,
            overflow_index_buffer,
        }
    }

    /// Evaluate root-level GodotGrass wind/clump/noise once per visible blade.
    /// The old faithful vertex path repeats this work for every mesh vertex (11x on
    /// a high blade). This compute pass writes a compact 44-byte transient record
    /// that the optimized vertex shader consumes with no noise texture lookups.
    pub fn encode_prepare(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        map: &GrassMapGpu,
        prepared: &GrassPreparedDraw,
        weather_bind_group: &wgpu::BindGroup,
    ) {
        if !prepared.gpu_precompute || prepared.total_visible == 0 {
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("GodotGrass visible blade preparation"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.prepare_pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_bind_group(2, weather_bind_group, &[]);
        for (chunk_index, chunk) in prepared.chunks.iter().enumerate() {
            let count = chunk
                .low_count
                .saturating_add(chunk.mid_count)
                .saturating_add(chunk.high_count);
            if count == 0 {
                continue;
            }
            pass.set_bind_group(1, &map.instance_chunks[chunk_index].prepare_bind_group, &[]);
            pass.dispatch_workgroups((count + PREPARE_WORKGROUP_SIZE - 1) / PREPARE_WORKGROUP_SIZE, 1, 1);
        }
    }

    pub fn draw_shadow<'pass>(
        &'pass self,
        pass: &mut wgpu::RenderPass<'pass>,
        shadow_caster_bind_group: &'pass wgpu::BindGroup,
        map: &'pass GrassMapGpu,
        prepared: &'pass GrassPreparedDraw,
        bevy_reverse_z: bool,
    ) {
        // The shadow path deliberately depends on the optimized prepared buffer.
        // If precompute is disabled for A/B diagnostics, skip the extra caster
        // rather than re-running source wind/noise math in a second vertex pass.
        if !prepared.gpu_precompute || prepared.high_total == 0 {
            return;
        }
        pass.set_pipeline(if bevy_reverse_z { &self.bevy_shadow_pipeline } else { &self.shadow_pipeline });
        pass.set_bind_group(0, shadow_caster_bind_group, &[]);
        pass.set_bind_group(1, &map.prepared_bind_group, &[]);
        pass.set_vertex_buffer(0, self.low_vertex_buffer.slice(..));
        pass.set_index_buffer(self.low_index_buffer.slice(..), wgpu::IndexFormat::Uint16);
        let first = prepared.low_total.saturating_add(prepared.mid_total);
        pass.draw_indexed(
            0..self.low_index_count,
            0,
            first..first.saturating_add(prepared.high_total),
        );
    }

    pub fn draw<'pass>(
        &'pass self,
        pass: &mut wgpu::RenderPass<'pass>,
        camera_bind_group: &'pass wgpu::BindGroup,
        shadow_bind_group: &'pass wgpu::BindGroup,
        map: &'pass GrassMapGpu,
        prepared: &'pass GrassPreparedDraw,
    ) -> GrassDrawStats {
        let stats = prepared.stats;
        if map.blade_count == 0 || prepared.total_visible == 0 {
            return stats;
        }

        pass.set_bind_group(0, camera_bind_group, &[]);
        pass.set_bind_group(1, &self.bind_group, &[]);
        pass.set_bind_group(2, shadow_bind_group, &[]);

        if prepared.gpu_precompute {
            pass.set_pipeline(&self.prepared_pipeline);
            pass.set_bind_group(3, &map.prepared_bind_group, &[]);

            // Grass is opaque/depth-writing. High-detail blades are overwhelmingly
            // the near population, then mid, then low. Draw near geometry first so
            // early-Z can reject covered farther fragments. Keep the old order
            // available for same-camera A/B measurement.
            if prepared.front_to_back {
                if prepared.high_total != 0 {
                    pass.set_vertex_buffer(0, self.high_vertex_buffer.slice(..));
                    pass.set_index_buffer(
                        self.high_index_buffer.slice(..),
                        wgpu::IndexFormat::Uint16,
                    );
                    pass.draw_indexed(
                        0..self.high_index_count,
                        0,
                        prepared.low_total + prepared.mid_total
                            ..prepared.low_total + prepared.mid_total + prepared.high_total,
                    );
                }
                if prepared.mid_total != 0 {
                    pass.set_vertex_buffer(0, self.mid_vertex_buffer.slice(..));
                    pass.set_index_buffer(
                        self.mid_index_buffer.slice(..),
                        wgpu::IndexFormat::Uint16,
                    );
                    pass.draw_indexed(
                        0..self.mid_index_count,
                        0,
                        prepared.low_total..prepared.low_total + prepared.mid_total,
                    );
                }
                if prepared.low_total != 0 {
                    pass.set_vertex_buffer(0, self.low_vertex_buffer.slice(..));
                    pass.set_index_buffer(
                        self.low_index_buffer.slice(..),
                        wgpu::IndexFormat::Uint16,
                    );
                    pass.draw_indexed(0..self.low_index_count, 0, 0..prepared.low_total);
                }
            } else {
                if prepared.low_total != 0 {
                    pass.set_vertex_buffer(0, self.low_vertex_buffer.slice(..));
                    pass.set_index_buffer(
                        self.low_index_buffer.slice(..),
                        wgpu::IndexFormat::Uint16,
                    );
                    pass.draw_indexed(0..self.low_index_count, 0, 0..prepared.low_total);
                }
                if prepared.mid_total != 0 {
                    pass.set_vertex_buffer(0, self.mid_vertex_buffer.slice(..));
                    pass.set_index_buffer(
                        self.mid_index_buffer.slice(..),
                        wgpu::IndexFormat::Uint16,
                    );
                    pass.draw_indexed(
                        0..self.mid_index_count,
                        0,
                        prepared.low_total..prepared.low_total + prepared.mid_total,
                    );
                }
                if prepared.high_total != 0 {
                    pass.set_vertex_buffer(0, self.high_vertex_buffer.slice(..));
                    pass.set_index_buffer(
                        self.high_index_buffer.slice(..),
                        wgpu::IndexFormat::Uint16,
                    );
                    pass.draw_indexed(
                        0..self.high_index_count,
                        0,
                        prepared.low_total + prepared.mid_total
                            ..prepared.low_total + prepared.mid_total + prepared.high_total,
                    );
                }
            }
            return stats;
        }

        // Fallback for an unusually large visible set that exceeds the compact
        // prepared buffer. This retains the exact faithful per-vertex source math.
        pass.set_pipeline(&self.pipeline);
        let visible_buffer = prepared
            .overflow_index_buffer
            .as_ref()
            .unwrap_or(&map.visible_index_buffer);
        let stride = std::mem::size_of::<u32>() as u64;
        for (chunk_index, chunk_draw) in prepared.chunks.iter().enumerate() {
            if chunk_draw.low_count == 0 && chunk_draw.mid_count == 0 && chunk_draw.high_count == 0 {
                continue;
            }
            pass.set_bind_group(3, &map.instance_chunks[chunk_index].bind_group, &[]);
            if prepared.front_to_back {
                if chunk_draw.high_count != 0 {
                    let begin = u64::from(chunk_draw.high_offset) * stride;
                    let end = begin + u64::from(chunk_draw.high_count) * stride;
                    pass.set_vertex_buffer(0, self.high_vertex_buffer.slice(..));
                    pass.set_vertex_buffer(1, visible_buffer.slice(begin..end));
                    pass.set_index_buffer(
                        self.high_index_buffer.slice(..),
                        wgpu::IndexFormat::Uint16,
                    );
                    pass.draw_indexed(0..self.high_index_count, 0, 0..chunk_draw.high_count);
                }
                if chunk_draw.mid_count != 0 {
                    let begin = u64::from(chunk_draw.mid_offset) * stride;
                    let end = begin + u64::from(chunk_draw.mid_count) * stride;
                    pass.set_vertex_buffer(0, self.mid_vertex_buffer.slice(..));
                    pass.set_vertex_buffer(1, visible_buffer.slice(begin..end));
                    pass.set_index_buffer(
                        self.mid_index_buffer.slice(..),
                        wgpu::IndexFormat::Uint16,
                    );
                    pass.draw_indexed(0..self.mid_index_count, 0, 0..chunk_draw.mid_count);
                }
                if chunk_draw.low_count != 0 {
                    let begin = u64::from(chunk_draw.low_offset) * stride;
                    let end = begin + u64::from(chunk_draw.low_count) * stride;
                    pass.set_vertex_buffer(0, self.low_vertex_buffer.slice(..));
                    pass.set_vertex_buffer(1, visible_buffer.slice(begin..end));
                    pass.set_index_buffer(
                        self.low_index_buffer.slice(..),
                        wgpu::IndexFormat::Uint16,
                    );
                    pass.draw_indexed(0..self.low_index_count, 0, 0..chunk_draw.low_count);
                }
            } else {
                if chunk_draw.low_count != 0 {
                    let begin = u64::from(chunk_draw.low_offset) * stride;
                    let end = begin + u64::from(chunk_draw.low_count) * stride;
                    pass.set_vertex_buffer(0, self.low_vertex_buffer.slice(..));
                    pass.set_vertex_buffer(1, visible_buffer.slice(begin..end));
                    pass.set_index_buffer(
                        self.low_index_buffer.slice(..),
                        wgpu::IndexFormat::Uint16,
                    );
                    pass.draw_indexed(0..self.low_index_count, 0, 0..chunk_draw.low_count);
                }
                if chunk_draw.mid_count != 0 {
                    let begin = u64::from(chunk_draw.mid_offset) * stride;
                    let end = begin + u64::from(chunk_draw.mid_count) * stride;
                    pass.set_vertex_buffer(0, self.mid_vertex_buffer.slice(..));
                    pass.set_vertex_buffer(1, visible_buffer.slice(begin..end));
                    pass.set_index_buffer(
                        self.mid_index_buffer.slice(..),
                        wgpu::IndexFormat::Uint16,
                    );
                    pass.draw_indexed(0..self.mid_index_count, 0, 0..chunk_draw.mid_count);
                }
                if chunk_draw.high_count != 0 {
                    let begin = u64::from(chunk_draw.high_offset) * stride;
                    let end = begin + u64::from(chunk_draw.high_count) * stride;
                    pass.set_vertex_buffer(0, self.high_vertex_buffer.slice(..));
                    pass.set_vertex_buffer(1, visible_buffer.slice(begin..end));
                    pass.set_index_buffer(
                        self.high_index_buffer.slice(..),
                        wgpu::IndexFormat::Uint16,
                    );
                    pass.draw_indexed(0..self.high_index_count, 0, 0..chunk_draw.high_count);
                }
            }
        }
        stats
    }

}

fn source_lod_count_fraction(distance_m: f32) -> f32 {
    // GodotGrass row-density targets are [1, .5, .25, .1, .02], and its
    // MultiMesh count is row_size^2. These are therefore the actual source
    // blade-count fractions. Preserve those targets but cross-fade the count
    // around each source threshold instead of doing a hard tile swap.
    let fractions = SOURCE_LOD_DENSITIES.map(|density| density * density);
    if distance_m < SOURCE_LOD0_METERS - 4.0 {
        fractions[0]
    } else if distance_m < SOURCE_LOD0_METERS + 4.0 {
        mix_cpu(fractions[0], fractions[1], smoothstep(SOURCE_LOD0_METERS - 4.0, SOURCE_LOD0_METERS + 4.0, distance_m))
    } else if distance_m < SOURCE_LOD1_METERS - 6.0 {
        fractions[1]
    } else if distance_m < SOURCE_LOD1_METERS + 6.0 {
        mix_cpu(fractions[1], fractions[2], smoothstep(SOURCE_LOD1_METERS - 6.0, SOURCE_LOD1_METERS + 6.0, distance_m))
    } else if distance_m < SOURCE_LOD2_METERS - 8.0 {
        fractions[2]
    } else if distance_m < SOURCE_LOD2_METERS + 8.0 {
        mix_cpu(fractions[2], fractions[3], smoothstep(SOURCE_LOD2_METERS - 8.0, SOURCE_LOD2_METERS + 8.0, distance_m))
    } else if distance_m < SOURCE_LOD3_METERS - 10.0 {
        fractions[3]
    } else if distance_m < SOURCE_LOD3_METERS + 10.0 {
        mix_cpu(fractions[3], fractions[4], smoothstep(SOURCE_LOD3_METERS - 10.0, SOURCE_LOD3_METERS + 10.0, distance_m))
    } else if distance_m < 180.0 {
        fractions[4]
    } else if distance_m < SOURCE_MAP_RADIUS_METERS {
        mix_cpu(
            fractions[4],
            0.0,
            smoothstep(180.0, SOURCE_MAP_RADIUS_METERS, distance_m),
        )
    } else {
        0.0
    }
}

fn source_mesh_weights(distance_m: f32, mid_lod: bool) -> (f32, f32, f32) {
    if !mid_lod {
        // Faithful two-mesh A/B path: source high -> low around 40 m.
        let low = smoothstep(SOURCE_LOD1_METERS - 8.0, SOURCE_LOD1_METERS + 8.0, distance_m);
        return (1.0 - low, 0.0, low);
    }

    // GPU-oriented extension of the source LOD: preserve the exact 9-triangle
    // blade close to the camera, migrate seed-staggered survivors to a 5-triangle
    // blade through 14..28 m, then migrate that middle blade to the source's exact
    // 1-triangle low mesh through 34..50 m. Density LOD remains unchanged.
    let high_to_mid = smoothstep(14.0, 28.0, distance_m);
    let mid_to_low = smoothstep(34.0, 50.0, distance_m);
    let high = 1.0 - high_to_mid;
    let low = high_to_mid * mid_to_low;
    let mid = high_to_mid * (1.0 - mid_to_low);
    (high, mid, low)
}

fn mix_cpu(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub(crate) fn pack_static_clump_alpha(
    position: [f32; 3],
    clump_data: &[u8],
    ground_valid: bool,
) -> u16 {
    let root_m3 = Vec3::from_array(position) * GODOT_WORLD_SCALE;
    let clump0 = sample_repeat_linear_r8(
        clump_data,
        CLUMP_NOISE_SIZE,
        root_m3.z * 0.075,
        root_m3.x * 0.075,
    );
    let clump1 = sample_repeat_linear_r8(
        clump_data,
        CLUMP_NOISE_SIZE,
        root_m3.x * 0.5,
        root_m3.z * 0.5,
    );
    let clump2 = sample_repeat_linear_r8(
        clump_data,
        CLUMP_NOISE_SIZE,
        -root_m3.z * 0.035,
        -root_m3.x * 0.035,
    );
    let q0 = (clump0.clamp(0.0, 1.0) * 31.0 + 0.5) as u16;
    let q1 = (clump1.clamp(0.0, 1.0) * 31.0 + 0.5) as u16;
    let q2 = (clump2.clamp(0.0, 1.0) * 31.0 + 0.5) as u16;
    q0 | (q1 << 5) | (q2 << 10) | (u16::from(ground_valid) << 15)
}

fn sample_repeat_linear_r8(data: &[u8], size: u32, u: f32, v: f32) -> f32 {
    let size_i = size as i32;
    let x = u.rem_euclid(1.0) * size as f32 - 0.5;
    let y = v.rem_euclid(1.0) * size as f32 - 0.5;
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let tx = x - x.floor();
    let ty = y - y.floor();
    let sample = |sx: i32, sy: i32| -> f32 {
        let ix = sx.rem_euclid(size_i) as usize;
        let iy = sy.rem_euclid(size_i) as usize;
        f32::from(data[iy * size as usize + ix]) / 255.0
    };
    let a = sample(x0, y0);
    let b = sample(x0 + 1, y0);
    let c = sample(x0, y0 + 1);
    let d = sample(x0 + 1, y0 + 1);
    let top = a + (b - a) * tx;
    let bottom = c + (d - c) * tx;
    top + (bottom - top) * ty
}

fn sphere_intersects_clip_frustum(center: Vec3, radius: f32, view_proj: Mat4) -> bool {
    // WebGPU clip volume: -w<=x<=w, -w<=y<=w, 0<=z<=w. Extract
    // the six world-space planes from matrix rows without normalizing them.
    let cols = view_proj.to_cols_array_2d();
    let row = |r: usize| Vec4::new(cols[0][r], cols[1][r], cols[2][r], cols[3][r]);
    let r0 = row(0);
    let r1 = row(1);
    let r2 = row(2);
    let r3 = row(3);
    let planes = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2];
    for plane in planes {
        let normal = plane.truncate();
        let normal_len = normal.length();
        if normal_len > 1.0e-6 && normal.dot(center) + plane.w < -radius * normal_len {
            return false;
        }
    }
    true
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn create_noise_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    data: &[u8],
) -> wgpu::Texture {
    // NoiseTexture2D generates mipmaps by default. Keep the same filtering behavior
    // rather than sampling only the full-resolution procedural texture at all ranges.
    let (mip_data, mip_level_count) = r8_mip_chain(width, height, data);
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &mip_data,
    )
}

fn create_rgba_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    width: u32,
    height: u32,
    data: &[u8],
) -> wgpu::Texture {
    let (mip_data, mip_level_count) = rgba8_mip_chain(width, height, data);
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &mip_data,
    )
}

fn rgba8_mip_chain(width: u32, height: u32, base: &[u8]) -> (Vec<u8>, u32) {
    let mut all = Vec::new();
    let mut current = base.to_vec();
    let mut width = width as usize;
    let mut height = height as usize;
    let mut levels = 0_u32;

    loop {
        all.extend_from_slice(&current);
        levels += 1;
        if width == 1 && height == 1 {
            break;
        }
        let next_width = (width / 2).max(1);
        let next_height = (height / 2).max(1);
        let mut next = vec![0_u8; next_width * next_height * 4];
        for y in 0..next_height {
            for x in 0..next_width {
                for channel in 0..4 {
                    let mut sum = 0_u32;
                    let mut count = 0_u32;
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let sx = (x * 2 + dx).min(width - 1);
                            let sy = (y * 2 + dy).min(height - 1);
                            sum += u32::from(current[(sy * width + sx) * 4 + channel]);
                            count += 1;
                        }
                    }
                    next[(y * next_width + x) * 4 + channel] =
                        ((sum + count / 2) / count) as u8;
                }
            }
        }
        current = next;
        width = next_width;
        height = next_height;
    }
    (all, levels)
}

fn r8_mip_chain(width: u32, height: u32, base: &[u8]) -> (Vec<u8>, u32) {
    let mut all = Vec::new();
    let mut current = base.to_vec();
    let mut width = width as usize;
    let mut height = height as usize;
    let mut levels = 0_u32;

    loop {
        all.extend_from_slice(&current);
        levels += 1;
        if width == 1 && height == 1 {
            break;
        }

        let next_width = (width / 2).max(1);
        let next_height = (height / 2).max(1);
        let mut next = vec![0_u8; next_width * next_height];
        for y in 0..next_height {
            for x in 0..next_width {
                let mut sum = 0_u32;
                let mut count = 0_u32;
                for dy in 0..2 {
                    for dx in 0..2 {
                        let sx = (x * 2 + dx).min(width - 1);
                        let sy = (y * 2 + dy).min(height - 1);
                        sum += u32::from(current[sx + sy * width]);
                        count += 1;
                    }
                }
                next[x + y * next_width] = ((sum + count / 2) / count) as u8;
            }
        }
        current = next;
        width = next_width;
        height = next_height;
    }

    (all, levels)
}

pub(crate) fn godot_clump_noise() -> Vec<u8> {
    // mat_grass.tres: FastNoiseLite noise_type=Cellular. All other fields use
    // Godot FastNoiseLite's defaults (seed 0, FBM 5 octaves, frequency .01, etc.).
    let mut noise = FastNoiseLite::with_seed(0);
    noise.set_noise_type(Some(NoiseType::Cellular));
    noise.set_frequency(Some(0.01));
    noise.set_fractal_type(Some(FractalType::FBm));
    noise.set_fractal_octaves(Some(5));
    noise.set_fractal_lacunarity(Some(2.0));
    noise.set_fractal_gain(Some(0.5));
    noise.set_cellular_distance_function(Some(CellularDistanceFunction::Euclidean));
    noise.set_cellular_return_type(Some(CellularReturnType::Distance));
    noise.set_cellular_jitter(Some(1.0));

    godot_seamless_noise(CLUMP_NOISE_SIZE, CLUMP_NOISE_SIZE, |x, y| {
        noise.get_noise_2d(x, y)
    })
}

fn godot_wind_noise() -> Vec<u8> {
    // mat_grass.tres exact settings: Perlin, frequency .0275, fractal gain .1,
    // domain warp amplitude 20 and domain warp frequency .005.
    let mut noise = FastNoiseLite::with_seed(0);
    noise.set_noise_type(Some(NoiseType::Perlin));
    noise.set_frequency(Some(0.0275));
    noise.set_fractal_type(Some(FractalType::FBm));
    noise.set_fractal_octaves(Some(5));
    noise.set_fractal_lacunarity(Some(2.0));
    noise.set_fractal_gain(Some(0.1));

    let mut warp = FastNoiseLite::with_seed(0);
    warp.set_domain_warp_type(Some(DomainWarpType::OpenSimplex2));
    warp.set_domain_warp_amp(Some(20.0));
    warp.set_frequency(Some(0.005));
    warp.set_fractal_type(Some(FractalType::DomainWarpProgressive));
    warp.set_fractal_octaves(Some(5));
    // Godot's domain-warp fractal defaults differ from ordinary fractal defaults.
    warp.set_fractal_lacunarity(Some(6.0));
    warp.set_fractal_gain(Some(0.5));

    godot_seamless_noise(WIND_NOISE_SIZE, WIND_NOISE_SIZE, |x, y| {
        let (warped_x, warped_y) = warp.domain_warp_2d(x, y);
        noise.get_noise_2d(warped_x, warped_y)
    })
}

/// Reproduce Godot NoiseTexture2D's default normalized seamless generation:
/// sample a 10% larger source image, normalize it globally to L8, quadrant-swap it,
/// then blend the overlap skirt across the center seams.
fn godot_seamless_noise<F>(width: u32, height: u32, mut sample: F) -> Vec<u8>
where
    F: FnMut(f32, f32) -> f32,
{
    let width = width as usize;
    let height = height as usize;
    let skirt_width = ((width as f32) * GODOT_SEAMLESS_SKIRT).floor().max(1.0) as usize;
    let skirt_height = ((height as f32) * GODOT_SEAMLESS_SKIRT).floor().max(1.0) as usize;
    let src_width = width + skirt_width;
    let src_height = height + skirt_height;
    let half_width = width / 2;
    let half_height = height / 2;
    let skirt_edge_x = half_width + skirt_width;
    let skirt_edge_y = half_height + skirt_height;

    let mut values = Vec::with_capacity(src_width * src_height);
    let mut min_value = f32::INFINITY;
    let mut max_value = f32::NEG_INFINITY;
    for y in 0..src_height {
        for x in 0..src_width {
            let value = sample(x as f32, y as f32);
            min_value = min_value.min(value);
            max_value = max_value.max(value);
            values.push(value);
        }
    }

    let mut source = vec![0_u8; src_width * src_height];
    if max_value != min_value {
        let inv_range = 1.0 / (max_value - min_value);
        for (dst, value) in source.iter_mut().zip(values.into_iter()) {
            *dst = (((value - min_value) * inv_range * 255.0).clamp(0.0, 255.0)) as u8;
        }
    }

    #[derive(Clone, Copy)]
    enum Modulo {
        Default,
        AltX,
        AltY,
        AltXy,
    }

    let read_source = |x: usize, y: usize, mode: Modulo| -> u8 {
        let (mod_x, mod_y) = match mode {
            Modulo::Default => (src_width, src_height),
            Modulo::AltX => (width, src_height),
            Modulo::AltY => (src_width, height),
            Modulo::AltXy => (width, height),
        };
        let sx = (x + half_width) % mod_x;
        let sy = (y + half_height) % mod_y;
        source[sx + sy * src_width]
    };

    let mut output = vec![0_u8; width * height];
    for y in 0..height {
        for x in 0..width {
            output[x + y * width] = read_source(x, y, Modulo::AltXy);
        }
    }

    // Vertical skirt, skipping the center square exactly as Godot does.
    for x in half_width..skirt_edge_x.min(width) {
        let alpha = (255.0
            * (1.0
                - smoothstep(
                    0.1,
                    0.9,
                    (x - half_width) as f32 / skirt_width as f32,
                ))) as u8;
        for y in 0..height {
            if (half_height..skirt_edge_y).contains(&y) {
                continue;
            }
            let idx = x + y * width;
            output[idx] = alpha_blend_u8(output[idx], read_source(x, y, Modulo::AltY), alpha);
        }
    }

    // Horizontal skirt, skipping the center square.
    for y in half_height..skirt_edge_y.min(height) {
        let alpha = (255.0
            * (1.0
                - smoothstep(
                    0.1,
                    0.9,
                    (y - half_height) as f32 / skirt_height as f32,
                ))) as u8;
        for x in 0..width {
            if (half_width..skirt_edge_x).contains(&x) {
                continue;
            }
            let idx = x + y * width;
            output[idx] = alpha_blend_u8(output[idx], read_source(x, y, Modulo::AltX), alpha);
        }
    }

    // Center square combines both skirt directions.
    for y in half_height..skirt_edge_y.min(height) {
        for x in half_width..skirt_edge_x.min(width) {
            let xpos = (255.0
                * (1.0
                    - smoothstep(
                        0.1,
                        0.9,
                        (x - half_width) as f32 / skirt_width as f32,
                    ))) as u8;
            let ypos = (255.0
                * (1.0
                    - smoothstep(
                        0.1,
                        0.9,
                        (y - half_height) as f32 / skirt_height as f32,
                    ))) as u8;
            let top_blend = alpha_blend_u8(
                read_source(x, y, Modulo::AltX),
                read_source(x, y, Modulo::Default),
                xpos,
            );
            let bottom_blend = alpha_blend_u8(
                read_source(x, y, Modulo::AltXy),
                read_source(x, y, Modulo::AltY),
                xpos,
            );
            output[x + y * width] = alpha_blend_u8(bottom_blend, top_blend, ypos);
        }
    }

    output
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn alpha_blend_u8(background: u8, foreground: u8, alpha: u8) -> u8 {
    let a = u16::from(alpha) + 1;
    let inv_a = 256 - u16::from(alpha);
    ((a * u16::from(foreground) + inv_a * u16::from(background)) >> 8) as u8
}

fn globals_for_environment(
    sun: Option<DirectionalSun>,
    cloud_wind_speed: f32,
    cloud_wind_direction: f32,
    realtime_sun_lighting: bool,
) -> GrassGlobals {
    let sun = sun.unwrap_or(DirectionalSun {
        direction: [-0.38, -0.84, -0.39],
        color: [1.0, 1.0, 1.0],
        intensity: 250.0,
    });
    let toward_sun = -Vec3::from_array(sun.direction).normalize_or_zero();
    let wind_angle = cloud_wind_direction.to_radians();
    let atmospheric_speed = cloud_wind_speed.max(0.0) * GODOT_WORLD_SCALE;
    GrassGlobals {
        sun_direction_strength: [
            toward_sun.x,
            toward_sun.y,
            toward_sun.z,
            if realtime_sun_lighting {
                (sun.intensity / 250.0).clamp(0.1, 4.0)
            } else {
                0.0
            },
        ],
        sun_color: [sun.color[0], sun.color[1], sun.color[2], 1.0],
        // Exact mat_grass.tres defaults.
        base_color: [0.05, 0.20, 0.01, 1.0],
        tip_color: [0.50, 0.50, 0.10, 1.0],
        sss_color: [1.0, 0.75, 0.10, 1.0],
        params: [
            GODOT_WORLD_SCALE,
            SOURCE_CLUMPING_FACTOR,
            wind_angle,
            SOURCE_REFERENCE_SPRITE_HEIGHT,
        ],
        weather_wind: [
            wind_angle.cos(),
            wind_angle.sin(),
            atmospheric_speed,
            atmospheric_speed * GRASS_CLOUD_WIND_INHERITANCE,
        ],
    }
}

fn high_blade_mesh() -> (Vec<BladeVertex>, Vec<u16>) {
    // Exact grass_high.obj profile from GodotGrass.
    let positions = [
        [-0.05, 0.0, 0.0],
        [0.05, 0.0, 0.0],
        [0.0, 0.75, 0.0],
        [-0.023333, 0.60, 0.0],
        [-0.034167, 0.45, 0.0],
        [-0.0425, 0.30, 0.0],
        [-0.0475, 0.15, 0.0],
        [0.0475, 0.15, 0.0],
        [0.0425, 0.30, 0.0],
        [0.034167, 0.45, 0.0],
        [0.023333, 0.60, 0.0],
    ];
    let vertices = positions
        .into_iter()
        .map(|position| BladeVertex {
            position,
            // OBJ import flips V in Godot. Root is V=1, tip is V=0.
            uv: [
                if position[0] < -0.001 {
                    0.0
                } else if position[0] > 0.001 {
                    1.0
                } else {
                    0.5
                },
                1.0 - position[1] / 0.75,
            ],
        })
        .collect();
    let indices = vec![
        3, 10, 2, // top triangle
        0, 1, 7, 0, 7, 6, // root segment
        6, 7, 8, 6, 8, 5, // segment 2
        5, 8, 9, 5, 9, 4, // segment 3
        4, 9, 10, 4, 10, 3, // segment 4
    ];
    (vertices, indices)
}

fn mid_blade_mesh() -> (Vec<BladeVertex>, Vec<u16>) {
    // Five-triangle middle LOD derived from the source high-blade silhouette.
    // Two ribbon segments preserve visible curvature; a single tip triangle keeps
    // the pointed shape. It is intentionally not used close enough to inspect.
    let positions = [
        [-0.05, 0.0, 0.0],
        [0.05, 0.0, 0.0],
        [-0.0425, 0.30, 0.0],
        [0.0425, 0.30, 0.0],
        [-0.023333, 0.60, 0.0],
        [0.023333, 0.60, 0.0],
        [0.0, 0.75, 0.0],
    ];
    let vertices = positions
        .into_iter()
        .map(|position| BladeVertex {
            position,
            uv: [
                if position[0] < -0.001 { 0.0 } else if position[0] > 0.001 { 1.0 } else { 0.5 },
                1.0 - position[1] / 0.75,
            ],
        })
        .collect();
    let indices = vec![
        0, 1, 3, 0, 3, 2, // root -> 0.30
        2, 3, 5, 2, 5, 4, // 0.30 -> 0.60
        4, 5, 6,           // pointed tip
    ];
    (vertices, indices)
}

fn low_blade_mesh() -> (Vec<BladeVertex>, Vec<u16>) {
    // Exact grass_low.obj: one triangle.
    (
        vec![
            BladeVertex {
                position: [-0.05, 0.0, 0.0],
                uv: [0.0, 1.0],
            },
            BladeVertex {
                position: [0.05, 0.0, 0.0],
                uv: [1.0, 1.0],
            },
            BladeVertex {
                position: [0.0, 0.75, 0.0],
                uv: [0.5, 0.0],
            },
        ],
        vec![0, 1, 2],
    )
}

fn create_grass_shadow_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    bevy_reverse_z: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("GodotGrass near-only shadow caster"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<BladeVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &BLADE_ATTRIBUTES,
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(if bevy_reverse_z {
                wgpu::CompareFunction::GreaterEqual
            } else {
                wgpu::CompareFunction::LessEqual
            }),
            stencil: Default::default(),
            bias: if bevy_reverse_z {
                wgpu::DepthBiasState::default()
            } else {
                wgpu::DepthBiasState {
                    constant: 2,
                    slope_scale: 2.0,
                    clamp: 0.0,
                }
            },
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: None,
        multiview_mask: None,
        cache: None,
    })
}

fn create_prepared_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    msaa_samples: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("GodotGrass GPU-prepared per-blade pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<BladeVertex>() as wgpu::BufferAddress,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &BLADE_ATTRIBUTES,
            }],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: msaa_samples,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    surface_format: wgpu::TextureFormat,
    msaa_samples: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("GodotGrass per-blade pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[
                wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<BladeVertex>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &BLADE_ATTRIBUTES,
                },
                wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<u32>() as wgpu::BufferAddress,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &VISIBLE_INDEX_ATTRIBUTES,
                },
            ],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: msaa_samples,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

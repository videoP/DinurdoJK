//! Shared change-triggered lightmap composition: authored-sun removal, indirect
//! tone and static replacement sunlight. Always rebuild from immutable sources.
use crate::renderer::{GpuImage, Renderer};
use wgpu::util::DeviceExt;

#[derive(Clone, Debug, PartialEq)]
struct Composition {
    generation: u64,
    radiance: [f32; 3],
    direction: [f32; 3],
    strength: f32,
    moving_strength: f32,
    mode: f32,
    adjust: [f32; 4],
    curve: [f32; 4],
    /// r_staticWorldCsm: also write the unshadowed-minus-baked sun amount so
    /// static cascades can replace baked visibility near the camera.
    static_csm: bool,
    /// Cached, ray-traced sky openness gates only the sky component.
    isolate_sky: bool,
    /// Sky illumination (Blue Water environment or vanilla sunlight curve):
    /// strength and quantized input key, so unchanged frames do not recompose.
    sky: Option<(f32, [i32; 6])>,
}

/// Shared BSP sky-bake inputs. Vanilla doesn't initialize Blue Water just to
/// sample a sun-elevation curve; the existing dummy atlas satisfies bindings.
pub(in crate::renderer) struct SkyInputs<'a> {
    pub normals: &'a [GpuImage],
    pub environment: Option<&'a ::blue_water_sky::environment::EnvironmentMap>,
    pub moon: Option<([f32; 3], f32)>,
    pub vanilla_toward_sun: Option<[f32; 3]>,
}

pub(in crate::renderer) struct LightmapCache {
    pub original: Vec<GpuImage>,
    corrected: Vec<GpuImage>,
    pipeline: wgpu::ComputePipeline,
    pub active: bool,
    /// AO/source writes invalidate the whole composition through this key.
    pub strength: Option<u32>,
    composition: Option<Composition>,
    smoothing: u8,
    signal: u8,
    /// Per-page R32Uint lightmap chart IDs at the original resolution (0 = none).
    charts: Vec<GpuImage>,
    has_charts: bool,
    /// Rgba8 per page: R = shadowed static sun / 4, G = 1 when absent.
    sun_aux: Vec<GpuImage>,
    dummy_aux: GpuImage,
    dummy_normals: GpuImage,
    dummy_environment: (GpuImage, wgpu::Sampler, wgpu::Buffer),
    /// Time-of-day drift recomposes in place at most once per second.
    sky_composed_at: Option<std::time::Instant>,
    vertex_sky: Option<VertexSkyGpu>,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Parameters {
    fit: [f32; 4],
    radiance_strength: [f32; 4],
    adjust: [f32; 4],
    curve: [f32; 4],
    sun: [f32; 4],
    direction_strength: [f32; 4],
    smoothing: [f32; 4],
    sky: [f32; 4],
    moon: [f32; 4],
}

/// GPU-only, change-triggered sky gain for static LIGHTMAP_BY_VERTEX BSP
/// surfaces (including foliage/patches as well as compiled model materials). Original RGB is retained in a tiny immutable receiver list;
/// the existing indexed world buffer is rewritten only on sky changes, never
/// on draw. Nothing is added to the minimal world fragment/vertex pipelines.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct VertexSkyInput {
    gpu_index: u32,
    packed: u32,
    reserved: [u32; 2],
    authored_color: [f32; 4],
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct VertexSkySettings {
    moon: [f32; 4],
    flags: [u32; 4],
}
struct VertexSkyGpu {
    pipeline: wgpu::ComputePipeline,
    source: wgpu::Buffer,
    generation: u64,
    count: u32,
    /// Receiver offset of first inline *N RGB; use this for mover-only redo.
    inline_start: u32,
    active: bool,
    moon: Option<([f32; 3], f32)>,
    vanilla_toward_sun: Option<[f32; 3]>,
    strength: f32,
    isolate_sky: bool,
}
impl VertexSkyGpu {
    fn new(device: &wgpu::Device, generation: u64, samples: &[VertexSkyInput], static_vertex_count: usize) -> Self {
        let shader_text = format!("{}\n{}", include_str!("vertex_sky.wgsl"), include_str!("vanilla_sky_lighting.wgsl"));
        let module = blue_water_clouds::shaders::compose(
            &shader_text, "static BSP vertex sky bake", &[],
        ).expect("vertex sky bake shader");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("static BSP vertex sky bake"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module)),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("static BSP vertex sky bake"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let source = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("authored static vertex sky receivers"),
            contents: bytemuck::cast_slice(samples),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let inline_start = samples.partition_point(|s| (s.gpu_index as usize) < static_vertex_count) as u32;
        Self { pipeline, source, generation, count: samples.len() as u32, inline_start, active: false, moon: None, vanilla_toward_sun: None, strength: 0.0, isolate_sky: false }
    }

    fn replace_samples(&mut self, device: &wgpu::Device, generation: u64, samples: &[VertexSkyInput], static_vertex_count: usize) {
        // Keep the lazily compiled pipeline alive across sun/sky re-bakes.
        self.source = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("authored static vertex sky receivers"),
            contents: bytemuck::cast_slice(samples),
            usage: wgpu::BufferUsages::STORAGE,
        });
        self.generation = generation;
        self.count = samples.len() as u32;
        self.inline_start = samples.partition_point(|s| (s.gpu_index as usize) < static_vertex_count) as u32;
    }

    fn dispatch(
        &self, device: &wgpu::Device, queue: &wgpu::Queue, vertices: &wgpu::Buffer,
        environment: Option<&::blue_water_sky::environment::EnvironmentMap>,
        fallback: (&GpuImage, &wgpu::Sampler, &wgpu::Buffer),
        moon: Option<([f32; 3], f32)>, vanilla_toward_sun: Option<[f32; 3]>,
        active: bool, strength: f32, isolate_sky: bool, receiver_offset: u32,
    ) {
        if receiver_offset >= self.count { return; }
        let settings = VertexSkySettings {
            moon: vanilla_toward_sun.map_or_else(
                || moon.map_or([0.0; 4], |(d, intensity)| [d[0], d[1], d[2], intensity]),
                |d| [d[0], d[1], d[2], 0.0],
            ),
            flags: [u32::from(active), u32::from(moon.is_some()), strength.to_bits(),
                u32::from(vanilla_toward_sun.is_some()) | (u32::from(isolate_sky) << 1) | (receiver_offset << 2)],
        };
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("static vertex sky bake parameters"),
            contents: bytemuck::bytes_of(&settings),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("static vertex sky bake inputs"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.source.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: vertices.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(environment.map_or(&fallback.0.view, |e| &e.view)) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(environment.map_or(fallback.1, |e| &e.sampler)) },
                wgpu::BindGroupEntry { binding: 4, resource: environment.map_or(fallback.2, |e| &e.uniforms).as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: uniform.as_entire_binding() },
            ],
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("update baked vertex sky RGB"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("update baked vertex sky RGB"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups((self.count - receiver_offset).div_ceil(64), 1, 1);
        }
        queue.submit(Some(encoder.finish()));
    }
}

#[path = "baked_sun_shader.rs"]
mod composition_shader;
use composition_shader::cache_shader;

fn cache_module() -> Result<wgpu::naga::Module, String> {
    blue_water_clouds::shaders::compose(&cache_shader(), "JKA lightmap composition", &[])
}

fn sun_aux_images(device: &wgpu::Device, sizes: &[[u32; 2]]) -> Vec<GpuImage> {
    sizes.iter().map(|s| {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("composed static sun shadow and sky overlay gain"),
            size: wgpu::Extent3d { width: s[0], height: s[1], depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        GpuImage::new(texture, view)
    }).collect()
}

fn corrected_images(device: &wgpu::Device, sizes: &[[u32; 2]]) -> Vec<GpuImage> {
    sizes
        .iter()
        .map(|s| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("composed sunlight lightmap"),
                size: wgpu::Extent3d {
                    width: s[0],
                    height: s[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            GpuImage::new(texture, view)
        })
        .collect()
}

impl LightmapCache {
    pub(in crate::renderer) fn has_composition(&self) -> bool {
        self.composition.is_some()
    }

    pub(in crate::renderer) fn vertex_sky_active(&self) -> bool {
        self.vertex_sky.as_ref().is_some_and(|state| state.active)
    }

    /// A cold material remap replaces the immutable receiver list. Its next
    /// generation may numerically match the previous map's first generation.
    pub(in crate::renderer) fn invalidate_vertex_sky(&mut self) {
        if let Some(state) = self.vertex_sky.as_mut() {
            state.generation = u64::MAX;
        }
    }
    /// Per-page static sun shadow (R) and baked sky overlay gain (G/B/A).
    pub(in crate::renderer) fn sun_aux(&self) -> &[GpuImage] {
        &self.sun_aux
    }

    /// True only after a composed sky page with the overlay gain has been
    /// installed. Other composition modes leave overlay shaders unchanged.
    pub(in crate::renderer) fn sky_overlay_active(&self) -> bool {
        self.active && self.signal == 0 && !self.sun_aux.is_empty()
            && self.composition.as_ref().and_then(|c| c.sky).is_some_and(|s| s.0 > 0.0)
    }
    pub(in crate::renderer) fn static_csm_active(&self) -> bool {
        self.active && !self.sun_aux.is_empty()
            && self.composition.as_ref().is_some_and(|c| c.static_csm && c.mode > 0.0 && c.mode < 1.5)
    }
    /// `charts` are per-page original-resolution chart IDs; empty disables
    /// chart-aware reconstruction (pages without geometry, unit tests).
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, original: &[GpuImage], charts: &[Vec<u32>]) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shared lightmap composition"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(
                cache_module().expect("lightmap composition shader"),
            )),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba16Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 10,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 11,
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("shared lightmap composition"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let sizes: Vec<_> = original
            .iter()
            .map(|i| [i._texture.width(), i._texture.height()])
            .collect();
        let has_charts = !sizes.is_empty() && charts.len() == sizes.len()
            && charts.iter().zip(&sizes).all(|(c, s)| c.len() == s[0] as usize * s[1] as usize);
        let charts = sizes.iter().enumerate().map(|(page, &[w, h])| {
            let (size, data) = if has_charts { ([w, h], &charts[page][..]) } else { ([1, 1], &[0u32][..]) };
            let texture = device.create_texture_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: Some("lightmap chart ids"),
                    size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R32Uint,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                bytemuck::cast_slice(data),
            );
            let view = texture.create_view(&Default::default());
            GpuImage::new(texture, view)
        }).collect();
        let small = |label, format, usage, data: &[u8]| {
            let texture = device.create_texture_with_data(queue, &wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | usage,
                view_formats: &[],
            }, wgpu::util::TextureDataOrder::LayerMajor, data);
            let view = texture.create_view(&Default::default());
            GpuImage::new(texture, view)
        };
        let dummy_aux = small("unused static sun shadow amount", wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::STORAGE_BINDING, &[0; 4]);
        let dummy_normals = small("unused sky bake normals", wgpu::TextureFormat::R32Uint,
            wgpu::TextureUsages::COPY_DST, &[0; 4]);
        let dummy_environment = (
            small("unused sky environment", wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureUsages::COPY_DST, &[0; 8]),
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("unused sky environment"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("unused sky environment"),
                contents: &[0; 256],
                usage: wgpu::BufferUsages::UNIFORM,
            }),
        );
        Self {
            original: original.to_vec(),
            corrected: corrected_images(device, &sizes),
            pipeline,
            active: false,
            strength: None,
            composition: None,
            smoothing: 0,
            signal: 0,
            charts,
            has_charts,
            sun_aux: Vec::new(),
            dummy_aux,
            dummy_normals,
            dummy_environment,
            sky_composed_at: None,
            vertex_sky: None,
        }
    }

    /// Restore A's authored vertex RGB or compose B's traced sky gain. The
    /// source is cached by static bake generation; when B is disabled we use
    /// that same saved source to restore RGB before discarding its GPU buffer.
    fn compose_vertex_sky(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue,
        vertex_buffer: &wgpu::Buffer, original: &[crate::renderer::GpuVertex],
        samples: &[super::lightmap_sun::VertexSkySample], generation: u64,
        sky: Option<(Option<&::blue_water_sky::environment::EnvironmentMap>, Option<([f32; 3], f32)>, Option<[f32; 3]>, f32, bool)>,
        environment: Option<&::blue_water_sky::environment::EnvironmentMap>,
        _debug_alpha_one: bool,
    ) {
        if let Some((environment, moon, vanilla_toward_sun, strength, isolate_sky)) = sky {
            if !samples.is_empty() && self.vertex_sky.as_ref().is_none_or(|g| g.generation != generation) {
                // Indexed upload interns coincident vertices across BSP
                // batches. Writing one RGB slot from multiple workgroups would
                // be an unordered storage race, so each GPU index is unique.
                let mut unique = std::collections::BTreeMap::new();
                for sample in samples {
                    let gpu_index = sample.gpu_index;
                    // Static world vertices use the AO-adjusted immutable RGB.
                    // Inline models are outside that prefix and retain their
                    // original compiled vertex color in the bake receiver.
                    let authored_color = original.get(gpu_index as usize)
                        .map_or(sample.authored_color, |vertex| vertex.color);
                    unique.entry(gpu_index).or_insert(VertexSkyInput {
                        gpu_index, packed: sample.packed, reserved: [0; 2],
                        authored_color,
                    });
                }
                let gpu_samples: Vec<_> = unique.into_values().collect();
                if !gpu_samples.is_empty() {
                    if let Some(state) = self.vertex_sky.as_mut() {
                        state.replace_samples(device, generation, &gpu_samples, original.len());
                    } else {
                        self.vertex_sky = Some(VertexSkyGpu::new(device, generation, &gpu_samples, original.len()));
                    }
                    println!("[SUN LIGHTMAP] {} vertex-lit BSP model receivers gain sky bake B", gpu_samples.len());
                }
            }
            if let Some(state) = self.vertex_sky.as_mut() {
                state.dispatch(device, queue, vertex_buffer, environment,
                    (&self.dummy_environment.0, &self.dummy_environment.1, &self.dummy_environment.2),
                    moon, vanilla_toward_sun, true, strength, isolate_sky, 0);
                state.active = true;
                state.moon = moon;
                state.vanilla_toward_sun = vanilla_toward_sun;
                state.strength = strength;
                state.isolate_sky = isolate_sky;
            }
        } else if let Some(state) = self.vertex_sky.as_mut().filter(|g| g.active) {
            // Restore the original (AO-adjusted) vertex RGB, even with no Blue
            // Water atlas. The dummy bindings make the zero-gain kernel safe.
            state.dispatch(device, queue, vertex_buffer, environment,
                (&self.dummy_environment.0, &self.dummy_environment.1, &self.dummy_environment.2),
                None, None, false, 0.0, false, 0);
            state.active = false;
        }
    }

    fn reapply_vertex_sky(
        &self, device: &wgpu::Device, queue: &wgpu::Queue,
        vertices: &wgpu::Buffer,
        environment: Option<&::blue_water_sky::environment::EnvironmentMap>,
    ) {
        if let Some(state) = &self.vertex_sky {
            state.dispatch(device, queue, vertices, environment,
                (&self.dummy_environment.0, &self.dummy_environment.1, &self.dummy_environment.2),
                state.moon, state.vanilla_toward_sun, state.active, state.strength, state.isolate_sky, 0);
        }
    }

    /// A CPU-posed inline model overwrote only its own RGB slots. Repaint
    /// those, not every static-world vertex on each moving snapshot.
    fn reapply_inline_vertex_sky(
        &self, device: &wgpu::Device, queue: &wgpu::Queue,
        vertices: &wgpu::Buffer,
        environment: Option<&::blue_water_sky::environment::EnvironmentMap>,
    ) {
        if let Some(state) = self.vertex_sky.as_ref().filter(|state| state.active) {
            state.dispatch(device, queue, vertices, environment,
                (&self.dummy_environment.0, &self.dummy_environment.1, &self.dummy_environment.2),
                state.moon, state.vanilla_toward_sun, true, state.strength,
                state.isolate_sky, state.inline_start);
        }
    }

    #[cfg(test)]
    fn bake(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoding: crate::scene::BakedSunEncoding,
        radiance: [f32; 3],
        strength: f32,
    ) {
        self.compose(
            device,
            queue,
            Some(encoding),
            radiance,
            strength,
            None,
            0,
            0,
            &[],
            &[],
            &[],
            None,
        );
    }

    fn compose(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoding: Option<crate::scene::BakedSunEncoding>,
        radiance: [f32; 3],
        strength: f32,
        composition: Option<&Composition>,
        smoothing: u8,
        signal: u8,
        flux: &[GpuImage],
        local_flux: &[GpuImage],
        colors: &[[f32; 4]],
        sky: Option<SkyInputs<'_>>,
    ) {
        let sizes: Vec<_> = if composition.is_some() {
            flux.iter()
                .map(|i| [i._texture.width(), i._texture.height() / 2])
                .collect()
        } else {
            let original_sizes: Vec<_> = self.original.iter()
                .map(|i| [i._texture.width(), i._texture.height()])
                .collect();
            // 2x cached reconstruction is memory, not new lighting information.
            // Cap the extra FP16 storage at 64 MiB; use 1x if atlas/limits exceed it.
            let can_upsample = smoothing != 0
                && original_sizes.iter().all(|&[w, h]| {
                    w.saturating_mul(2) <= device.limits().max_texture_dimension_2d
                        && h.saturating_mul(2) <= device.limits().max_texture_dimension_2d
                })
                && original_sizes.iter().map(|&[w, h]| u64::from(w)*u64::from(h)*32)
                    .sum::<u64>() <= 64*1024*1024;
            original_sizes.into_iter().map(|[w, h]| {
                let factor = if can_upsample {2} else {1};
                [w*factor, h*factor]
            }).collect()
        };
        if self.corrected.len() != sizes.len() || self
            .corrected
            .iter()
            .zip(&sizes)
            .any(|(i, s)| i._texture.width() != s[0] || i._texture.height() != s[1])
        {
            self.corrected = corrected_images(device, &sizes);
        }
        let static_csm = composition.is_some_and(|c| c.static_csm);
        let sky_strength = composition.and_then(|c| c.sky).map_or(0.0, |(strength, _)| strength);
        if !static_csm && sky_strength <= 0.0 {
            self.sun_aux.clear();
        } else if self.sun_aux.len() != sizes.len() || self.sun_aux.iter().zip(&sizes)
            .any(|(i, s)| i._texture.width() != s[0] || i._texture.height() != s[1])
        {
            self.sun_aux = sun_aux_images(device, &sizes);
        }
        let sky = sky.filter(|_| sky_strength > 0.0);
        let colors_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("baked local colors"),
            contents: bytemuck::cast_slice(if colors.is_empty() {
                &[[0.0f32; 4]]
            } else {
                colors
            }),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("lightmap composition update"),
        });
        for (index, (original, corrected)) in self.original.iter().zip(&self.corrected).enumerate()
        {
            let fit = encoding.map_or([0.0, 1.0, 0.0, 1.0], |e| {
                [
                    e.exposure,
                    e.gamma,
                    4.0,
                    e.compensate
                        * if original._texture.format().is_srgb() {
                            1.0
                        } else {
                            -1.0
                        },
                ]
            });
            let params = Parameters {
                fit,
                radiance_strength: [radiance[0], radiance[1], radiance[2], strength],
                adjust: composition.map_or([1.0, 1.0, 0.0, 0.0], |c| c.adjust),
                curve: {
                    let mut value = composition.map_or([1.0, 1.0, 0.0, 0.0], |c| c.curve);
                    value[2] = colors.len() as f32;
                    value[3] = composition.map_or(0.0, |c| c.moving_strength);
                    value
                },
                sun: composition.map_or([0.0; 4], |c| {
                    [c.radiance[0], c.radiance[1], c.radiance[2], c.mode]
                }),
                direction_strength: composition.map_or([0.0; 4], |c| {
                    [c.direction[0], c.direction[1], c.direction[2], c.strength]
                }),
                smoothing: [smoothing as f32, signal as f32, if self.has_charts { 1.0 } else { 0.0 }, u8::from(composition.is_some_and(|c| c.isolate_sky)) as f32],
                sky: [
                    sky_strength,
                    if sky.as_ref().is_some_and(|s| s.normals.get(index).is_some()) {
                        if sky.as_ref().is_some_and(|s| s.vanilla_toward_sun.is_some()) { 2.0 } else { 1.0 }
                    } else { 0.0 },
                    if static_csm { 1.0 } else { 0.0 },
                    u8::from(sky.as_ref().is_some_and(|s| s.moon.is_some())) as f32,
                ],
                moon: sky.as_ref().map_or([0.0; 4], |s| {
                    s.vanilla_toward_sun.map_or_else(
                        || s.moon.map_or([0.0; 4], |(direction, intensity)| [direction[0], direction[1], direction[2], intensity]),
                        |d| [d[0], d[1], d[2], 0.0],
                    )
                }),
            };
            let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let fallback_local = original._texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("lightmap composition inputs"),
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(
                            local_flux.get(index).map_or(&fallback_local, |i| &i.view),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: colors_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&original.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&corrected.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(
                            &flux.get(index).unwrap_or(original).view,
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 6,
                        resource: wgpu::BindingResource::TextureView(&self.charts[index].view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 7,
                        resource: wgpu::BindingResource::TextureView(
                            &self.sun_aux.get(index).unwrap_or(&self.dummy_aux).view,
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 8,
                        resource: wgpu::BindingResource::TextureView(
                            &sky.as_ref().and_then(|s| s.normals.get(index)).unwrap_or(&self.dummy_normals).view,
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 9,
                        resource: wgpu::BindingResource::TextureView(
                            sky.as_ref().and_then(|s| s.environment).map_or(&self.dummy_environment.0.view, |e| &e.view),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: wgpu::BindingResource::Sampler(
                            sky.as_ref().and_then(|s| s.environment).map_or(&self.dummy_environment.1, |e| &e.sampler),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 11,
                        resource: sky.as_ref().and_then(|s| s.environment)
                            .map_or(&self.dummy_environment.2, |e| &e.uniforms)
                            .as_entire_binding(),
                    },
                ],
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("compose static sunlight"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(
                corrected._texture.width().div_ceil(8),
                corrected._texture.height().div_ceil(8),
                1,
            );
        }
        queue.submit([encoder.finish()]);
        self.strength = Some(strength.to_bits());
        self.composition = composition.cloned();
        self.smoothing = smoothing;
        self.signal = signal;
    }
}

impl Renderer {
    pub(in crate::renderer) fn baked_sun_cache_active(&self) -> bool {
        self.world
            .as_ref()
            .and_then(|w| w.baked_sun_cache.as_ref())
            // This flag tells the BSP shader that authored-sun processing was
            // already handled by the compositor. Filtering alone does not.
            .is_some_and(|c| c.active && (c.composition.is_some()
                || c.strength.is_some_and(|bits| f32::from_bits(bits) > 0.0)))
    }
    /// Static cascades replace baked visibility near the camera (r_staticWorldCsm).
    pub(in crate::renderer) fn static_world_csm_composed(&self) -> bool {
        self.world.as_ref()
            .and_then(|w| w.baked_sun_cache.as_ref())
            .is_some_and(|c| c.static_csm_active())
    }
    pub(in crate::renderer) fn static_sun_lightmap_active(&self) -> bool {
        self.world
            .as_ref()
            .and_then(|w| w.baked_sun_cache.as_ref())
            .is_some_and(|c| c.active && c.composition.as_ref().is_some_and(|c| c.mode > 0.0))
    }
    /// AO/debug reuploads overwrite the indexed vertex buffer. Restore B's
    /// already-traced sky colour without retracing, changing cvars or rebuilding
    /// any world pipeline. This is called only after an explicit reupload.
    pub(in crate::renderer) fn reapply_baked_vertex_sky(&self) {
        let Some(world) = self.world.as_ref() else { return; };
        let Some(cache) = world.baked_sun_cache.as_ref() else { return; };
        let environment = self.blue_water_sky.as_ref().map(|s| s.sky.environment());
        cache.reapply_vertex_sky(&self.device, &self.queue, &world.vertex_buffer, environment);
    }
    /// CPU-posed BMODEL RGB is reuploaded from authored data. Reapply only
    /// inline receivers; static-world sky RGB is unchanged.
    pub(in crate::renderer) fn reapply_baked_inline_vertex_sky(&self) {
        let Some(world) = self.world.as_ref() else { return; };
        let Some(cache) = world.baked_sun_cache.as_ref() else { return; };
        let environment = self.blue_water_sky.as_ref().map(|s| s.sky.environment());
        cache.reapply_inline_vertex_sky(&self.device, &self.queue, &world.vertex_buffer, environment);
    }
    /// All callers meet here: sun changes, removal controls, AO uploads and
    /// worker completion only invalidate inputs; warm frames allocate nothing.
    pub(in crate::renderer) fn sync_baked_sun_lightmap_cache(&mut self) {
        let sun = self.relit_celestial();
        let moon_light = if self.blue_water_night() {
            let direction = self.blue_water_moon_toward();
            Some((
                direction.unwrap_or(crate::renderer::Vec3::Y).to_array(),
                if direction.is_some() {
                    self.blue_water_environment.effective_moon_intensity()
                } else { 0.0 },
            ))
        } else { None };
        let overcast_direct = self.relight_direct_fraction();
        let requested_removal = self.effective_baked_sun_removal();
        let Some(world) = self.world.as_ref() else {
            return;
        };
        let supported = (self.baked_sun_cache || (self.added_map_light_sources() != 0)) && !world.source_map;
        let removal = supported
            && world.baked_sun_encoding.is_some()
            && world.baked_sun_radiance.is_some()
            && requested_removal > 0.0
            && self.baked_sun_debug_mode == 0;
        let sun_ready = self.baked_sun_cache
            && !self.sun_bake_deferred
            && world.static_sun.ready()
            && (removal || requested_removal <= 0.0 || world.sun.is_none());
        let local_ready = (self.added_map_light_sources() != 0) && world.static_sun.local_ready();
        let explicit_sun = self.sun_lighting || (self.sun_override && self.blue_water_night());
        // The vanilla sky's daylight curve is independent of Blue Water's
        // atmosphere. Keep the precomputed openness and smoothed normals from
        // the existing static sun bake; evaluate gain only on input changes.
        let vanilla_toward_sun = (self.sky_backend != 1 && self.sun_override).then(||
            (-crate::renderer::Vec3::from_array(self.active_sun().direction).normalize_or_zero()).to_array()
        );
        let sky_light = if sun_ready && self.sun_override && self.sky_world_bake_ab != 0
            && self.sky_world_lighting_strength > 0.0
            && (!world.static_sun.sky_normals.is_empty() || !world.static_sun.vertex_sky.is_empty()) {
            let q = |v: f32, scale: f32| (v * scale).round() as i32;
            if let Some(toward) = vanilla_toward_sun {
                Some((self.sky_world_lighting_strength, [
                    q(toward[0], 200.0), q(toward[1], 200.0), q(toward[2], 200.0), 0, 0, 2,
                ]))
            } else {
                self.blue_water_sky.as_ref().map(|atmosphere| {
                    let light = atmosphere.sky.light();
                    let (direction, intensity, color_delta, moon) = if let Some((direction, intensity)) = moon_light {
                        (direction, intensity, 0.70 - 1.0, 1)
                    } else {
                        ([light.direction.x, light.direction.y, light.direction.z],
                         light.intensity, light.color.x - light.color.z, 0)
                    };
                    (self.sky_world_lighting_strength, [
                        q(direction[0], 200.0), q(direction[1], 200.0), q(direction[2], 200.0),
                        q(intensity.max(1e-6).log2(), 16.0), q(color_delta, 100.0), moon,
                    ])
                })
            }
        } else { None };
        let composition = (supported
            && (sun_ready || local_ready)
            && self.baked_sun_debug_mode == 0)
            .then(|| Composition {
                generation: world.static_sun.generation,
                radiance: sun.color.map(|c| c * sun.intensity.max(0.0) / 255.0),
                direction: sun.direction,
                strength: if self.cascaded_shadow_mode == crate::renderer::DynamicShadowsMode::CascadedShadowMapsEntities {
                    0.0
                } else { self.sun_shadow_strength * overcast_direct },
                // The legacy/indirect mode must soften moving and static
                // contrast together. Explicit-sun mode instead receives the
                // already attenuated radiance from relit_celestial().
                moving_strength: self.sun_shadow_strength * overcast_direct,
                mode: if !sun_ready {
                    0.0
                } else if explicit_sun {
                    1.0
                } else {
                    2.0
                },
                adjust: [
                    self.baked_indirect_scale,
                    self.baked_indirect_saturation,
                    self.baked_sun_edge_softness,
                    self.baked_sun_direct_limit,
                ],
                curve: [
                    self.baked_indirect_power,
                    self.baked_indirect_highlight_preserve,
                    0.0,
                    0.0,
                ],
                isolate_sky: self.sky_world_light_isolation,
                static_csm: sun_ready && explicit_sun
                    && self.cascaded_shadow_mode == crate::renderer::DynamicShadowsMode::CascadedShadowMaps,
                sky: sky_light,
            });
        // Smoothing is also available without relighting, including vanilla maps.
        let smoothing = self.lightmap_smoothing.min(2);
        let signal = self.lightmap_signal.min(8);
        let desired = removal || composition.is_some()
            || ((smoothing != 0 || signal != 0) && !world.source_map && !world.lightmaps.is_empty());
        let strength = if removal { requested_removal } else { 0.0 };
        let same_inputs = |c: &LightmapCache| {
            !desired || (c.strength == Some(strength.to_bits()) && c.smoothing == smoothing && c.signal == signal)
        };
        // A sky-light-only change rewrites the same composed pages in place: no
        // rebinding, pipeline change or TAA history reset.
        let mut sky_only = false;
        match world.baked_sun_cache.as_ref() {
            Some(c) if c.active == desired && c.composition == composition && same_inputs(c) => return,
            Some(c) if c.active && desired && same_inputs(c) => {
                if let (Some(old), Some(new)) = (c.composition.as_ref(), composition.as_ref()) {
                    sky_only = Composition { sky: new.sky, ..old.clone() } == *new
                        && old.sky.is_some() && new.sky.is_some();
                    let drift = sky_only && old.sky.map(|s| s.0) == new.sky.map(|s| s.0);
                    if drift && c.sky_composed_at.is_some_and(|t| t.elapsed().as_secs_f32() < 1.0) {
                        return;
                    }
                }
            }
            None if !desired => return,
            _ => {}
        }
        let mut world = self.world.take().unwrap();
        let mut rebound = false;
        if desired {
            let c = world.baked_sun_cache.get_or_insert_with(|| {
                let sizes: Vec<_> = world.lightmaps.iter()
                    .map(|i| [i._texture.width(), i._texture.height()])
                    .collect();
                let charts = world.static_sun.geometry.as_ref()
                    .map_or_else(Vec::new, |g| g.lightmap_charts(&sizes));
                LightmapCache::new(&self.device, &self.queue, &world.lightmaps, &charts)
            });
            c.compose(
                &self.device,
                &self.queue,
                if removal {
                    world.baked_sun_encoding
                } else {
                    None
                },
                world.baked_sun_radiance.unwrap_or([0.0; 3]),
                strength,
                composition.as_ref(),
                smoothing,
                signal,
                &world.static_sun.flux,
                if local_ready {
                    &world.static_sun.local_flux
                } else {
                    &[]
                },
                &if local_ready {
                    world
                        .static_sun
                        .local_lights
                        .iter()
                        .map(|(_, l)| [l.color[0], l.color[1], l.color[2], 1.0])
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                },
                sky_light.map(|_| SkyInputs {
                    normals: &world.static_sun.sky_normals,
                    environment: self.blue_water_sky.as_ref().filter(|_| self.sky_backend == 1)
                        .map(|atmosphere| atmosphere.sky.environment()),
                    moon: if self.sky_backend == 1 { moon_light } else { None },
                    vanilla_toward_sun,
                }),
            );
            let environment = self.blue_water_sky.as_ref().map(|atmosphere| atmosphere.sky.environment());
            c.compose_vertex_sky(
                &self.device, &self.queue, &world.vertex_buffer,
                &world.static_ao_gpu_vertices,
                &world.static_sun.vertex_sky, world.static_sun.generation,
                sky_light.map(|_| (
                    environment.filter(|_| self.sky_backend == 1),
                    if self.sky_backend == 1 { moon_light } else { None },
                    vanilla_toward_sun,
                    self.sky_world_lighting_strength, self.sky_world_light_isolation,
                )),
                environment, self.debug_world_vertex_alpha_one,
            );
            c.sky_composed_at = sky_light.map(|_| std::time::Instant::now());
            if sky_only {
                self.world = Some(world);
                return;
            }
            world.lightmaps = c.corrected.clone();
            c.active = true;
            rebound = true;
            println!(
                "[SUN LIGHTMAP] composed {} page(s), removal {:.3}, static sunlight {}, local emitters {}, smoothing {} (chart-aware {}), signal {}, static-world CSM {}, sky world illumination {}",
                world.lightmaps.len(),
                strength,
                sun_ready, if local_ready {world.static_sun.local_lights.len()} else {0}, smoothing, c.has_charts, signal,
                c.static_csm_active(), sky_light.map_or(0.0, |s| s.0)
            );
        } else if let Some(c) = world.baked_sun_cache.as_mut() {
            world.lightmaps = c.original.clone();
            c.active = false;
            c.composition = None;
            let environment = self.blue_water_sky.as_ref().map(|a| a.sky.environment());
            c.compose_vertex_sky(
                &self.device, &self.queue, &world.vertex_buffer,
                &world.static_ao_gpu_vertices,
                &[], world.static_sun.generation, None, environment,
                self.debug_world_vertex_alpha_one,
            );
            rebound = true;
        }
        if rebound {
            world.local_shadows.selection_position = None;
            self.rebuild_world_surface_bind_groups(&mut world);
        }
        self.world = Some(world);
        self.upload_light_buffer();
        self.update_lighting_settings();
        self.rebuild_frame_plan();
        self.activate_world_pipeline_variant();
        self.sync_runtime_variant_flags();
        self.history_valid = false;
    }
}

#[cfg(test)]

mod tests {
    use super::*;

    #[test]
    fn vertex_sky_bake_shader_and_packed_vertex_layout_validate() {
        assert_eq!(std::mem::size_of::<crate::renderer::GpuVertex>(), 15 * 4,
            "vertex sky compute RGB offsets require the packed indexed layout");
        assert_eq!(std::mem::size_of::<VertexSkyInput>(), 32);
        let shader_text = format!("{}\n{}", include_str!("vertex_sky.wgsl"), include_str!("vanilla_sky_lighting.wgsl"));
        let sky = blue_water_clouds::shaders::compose(
            &shader_text, "static BSP vertex sky bake", &[],
        ).unwrap_or_else(|e| panic!("vertex sky: {e}"));
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        ).validate(&sky).unwrap_or_else(|e| panic!("vertex sky: {e:?}"));
    }

    #[test]
    fn lightmap_composition_and_receivers_validate() {
        let compositor = cache_module().unwrap_or_else(|e| panic!("lightmap compositor: {e}"));
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&compositor)
        .unwrap_or_else(|e| panic!("lightmap compositor: {e:?}"));
        let world_light = blue_water_clouds::shaders::compose(
            include_str!("../environment/blue_water_scene.wgsl"), "native world light", &[],
        ).unwrap_or_else(|e| panic!("native world light: {e}"));
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&world_light)
        .unwrap_or_else(|e| panic!("native world light: {e:?}"));
        for (name, source) in [
            (
                "full world",
                crate::renderer::compose_world_shader(include_str!("../../bsp.wgsl")),
            ),
            (
                "lean world",
                crate::renderer::compose_world_shader(include_str!("../../bsp_lean.wgsl")),
            ),
        ] {
            let module = wgpu::naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&source)));
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&source)));
        }
    }

    #[test]
    #[ignore = "requires a Vulkan GPU"]
    fn lightmap_composition_bakes_sun_without_accumulation_on_gpu() {
        crate::renderer::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance.request_adapter(&Default::default()).await.unwrap();
            let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let original = crate::renderer::upload_texture(
                &device,
                &queue,
                &crate::materials::TextureData {
                    label: "sun composition source".into(),
                    source: None,
                    width: 1,
                    height: 1,
                    rgba: (vec![255; 4]).into(),
                    rgba16f: None,
                    mip_level_count: 1,
                    clamp: true,
                    srgb: true,
                },
            );
            let mut cache = LightmapCache::new(&device, &queue, &[original], &[]);
            // Two independent-resolution sunlight texels over one original texel.
            let texture = device.create_texture_with_data(
                &queue,
                &wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: 2,
                        height: 2,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                bytemuck::cast_slice(&[1.0f32, 0.0, 1.0, 1.0]),
            );
            let view = texture.create_view(&Default::default());
            let flux = vec![GpuImage::new(texture, view)];
            let half = |bits: u16| {
                let sign = if bits & 32768 != 0 { -1.0 } else { 1.0 };
                let e = ((bits >> 10) & 31) as i32;
                let m = (bits & 1023) as f32 / 1024.0;
                sign * if e == 0 {
                    m * 2.0f32.powi(-14)
                } else {
                    (1.0 + m) * 2.0f32.powi(e - 15)
                }
            };
            let local_texture = device.create_texture_with_data(
                &queue,
                &wgpu::TextureDescriptor {
                    label: Some("two independent local sources"),
                    size: wgpu::Extent3d {
                        width: 2,
                        height: 1,
                        depth_or_array_layers: 2,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                bytemuck::cast_slice(&[0.2f32, 0.4, 0.6, 0.1]),
            );
            let local_view = local_texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
            let local_flux = vec![GpuImage::new(local_texture, local_view)];
            let colors = [[1.0f32, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]];
            for (mode, local_enabled, world_strength, moving_strength) in [
                (1.0, false, 0.3, 0.6),
                (1.0, true, 0.3, 0.6),
                (0.0, true, 0.3, 0.6),
                (2.0, true, 0.3, 0.6),
                (2.0, true, 0.0, 0.0),
                (2.0, true, 0.0, 0.25),
                (2.0, true, 0.0, 1.0),
                (1.0, false, 0.3, 0.6),
            ] {
                let composition = Composition {
                    generation: 1,
                    radiance: [0.6, 0.4, 0.2],
                    direction: [0.0, -1.0, 0.0],
                    strength: world_strength,
                    moving_strength,
                    mode,
                    adjust: [1.0, 1.0, 0.0, 0.0],
                    curve: [1.0, 1.0, 0.0, 0.0],
                    static_csm: false,
                    isolate_sky: false,
                    sky: None,
                };
                cache.compose(
                    &device,
                    &queue,
                    None,
                    [0.0; 3],
                    0.0,
                    Some(&composition),
                    0,
                    0,
                    &flux,
                    if local_enabled { &local_flux } else { &[] },
                    if local_enabled { &colors } else { &[] },
                    None,
                );
                assert_eq!(cache.corrected[0]._texture.width(), 2);
                let read = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: 256,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                encoder.copy_texture_to_buffer(
                    cache.corrected[0]._texture.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &read,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(256),
                            rows_per_image: Some(1),
                        },
                    },
                    wgpu::Extent3d {
                        width: 2,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                );
                queue.submit([encoder.finish()]);
                read.slice(..)
                    .map_async(wgpu::MapMode::Read, |r| r.unwrap());
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    })
                    .unwrap();
                let bytes = read.slice(..).get_mapped_range().unwrap();
                for x in 0..2 {
                    for c in 0..4 {
                        let incident = if x == 0 { 1.0 } else { 0.0 };
                        let mut expected = if c == 3 {
                            if mode == 0.0 {
                                0.0
                            } else if mode == 1.0 {
                                incident
                            } else {
                                moving_strength * incident / (1.0 - world_strength * (1.0 - incident))
                            }
                        } else if mode == 0.0 {
                            1.0
                        } else if mode == 1.0 {
                            1.0 + composition.radiance[c] * incident
                        } else {
                            1.0 - world_strength * (1.0 - incident)
                        };
                        if local_enabled && c < 3 {
                            expected += match (x, c) {
                                (0, 0) => 0.2,
                                (1, 0) => 0.4,
                                (0, 2) => 0.6,
                                (1, 2) => 0.1,
                                _ => 0.0,
                            };
                        }
                        let offset = x * 8 + c * 2;
                        let actual = half(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]));
                        assert!(
                            (actual - expected).abs() < 0.002,
                            "mode {mode}, pixel {x}, channel {c}: {actual} vs {expected}"
                        );
                    }
                }
            }
            let correction = format!(
                r#"
struct PointLight {{ position_radius:vec4<f32>,color_intensity:vec4<f32>,emitter:vec4<f32>,shape:vec4<f32>,shadow:vec4<f32> }}
struct Settings {{ baked_sun_debug_mode:vec4<u32>,baked_indirect_curve:vec4<f32>,values:vec4<u32> }}
struct VertexOut {{ shell_kind:u32,lightmap_uv:vec2<f32>,world_position:vec3<f32>,world_normal:vec3<f32> }}
var<private> lighting_settings:Settings;var<private> dynamic_lights:array<PointLight,2>;var<private> test_case:u32;
@group(0) @binding(0) var<storage,read_write> output:array<vec4<f32>>;
@group(0) @binding(1) var baked_local_texture:texture_2d_array<f32>;
fn material_flags()->u32 {{return 2u;}} fn material_tc_gen()->u32 {{return 0u;}}
fn dynamic_light_debug_enabled()->bool {{return false;}}
fn lamp_raster_receiver_culling_safe()->bool {{return true;}}
fn baked_local_moving_maybe(light:PointLight,position:vec3<f32>)->bool {{return true;}}
fn sample_local_shadow_visibility(light:PointLight,position:vec3<f32>,normal:vec3<f32>)->f32 {{return 1.0;}}
fn baked_local_moving_visibility(light:PointLight,position:vec3<f32>,normal:vec3<f32>)->f32 {{
    let layer=u32(light.shape.w)-1u;return select(1.0,0.0,(test_case & (1u<<layer))!=0u);
}}
{}
@compute @workgroup_size(1) fn main(@builtin(global_invocation_id) id:vec3<u32>) {{
    test_case=id.x;lighting_settings.baked_sun_debug_mode=vec4(0u,1u,2u,1u);
    lighting_settings.baked_indirect_curve=vec4(1.0,1.0,513.0,0.0);lighting_settings.values.y=2u;
    dynamic_lights[0].color_intensity=vec4(1.0,0.0,0.0,1.0);dynamic_lights[0].shape.w=1.0;dynamic_lights[0].shadow.x=1.0;
    dynamic_lights[1].color_intensity=vec4(0.0,0.0,1.0,1.0);dynamic_lights[1].shape.w=2.0;dynamic_lights[1].shadow.x=2.0;
    dynamic_lights[0].position_radius.w=100.0;dynamic_lights[1].position_radius.w=100.0;
    var input:VertexOut;input.lightmap_uv=vec2(0.25,0.5);
    output[id.x]=vec4(shade_baked_local(vec3(1.2,1.0,1.6),input),1.0);
}}
"#,
                include_str!("lightmap_local.wgsl")
            );
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("actual local moving-shadow correction"),
                source: wgpu::ShaderSource::Wgsl(correction.into()),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: None,
                layout: None,
                module: &module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });
            let output = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let read = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 64,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: output.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&local_flux[0].view),
                    },
                ],
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups(4, 1, 1);
            }
            encoder.copy_buffer_to_buffer(&output, 0, &read, 0, 64);
            queue.submit([encoder.finish()]);
            read.slice(..)
                .map_async(wgpu::MapMode::Read, |r| r.unwrap());
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            let bytes = read.slice(..).get_mapped_range().unwrap();
            for (case, expected) in [
                [1.2f32, 1.0, 1.6],
                [1.0, 1.0, 1.6],
                [1.2, 1.0, 1.0],
                [1.0, 1.0, 1.0],
            ]
            .iter()
            .enumerate()
            {
                for channel in 0..3 {
                    let offset = (case * 4 + channel) * 4;
                    let value = f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
                    assert!(
                        (value - expected[channel]).abs() < 0.0001,
                        "case {case} channel {channel}: {value} vs {}",
                        expected[channel]
                    );
                }
            }
            assert!(scope.pop().await.is_none());
        });
    }

    #[test]
    #[ignore = "requires a Vulkan GPU"]
    fn baked_sun_cache_preserves_source_and_rebuilds_without_accumulation_on_gpu() {
        crate::renderer::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance.request_adapter(&Default::default()).await.unwrap();
            let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
            for exposure in [0.0_f32, 260.0] {
                let sun = if exposure == 0.0 { 140.0 } else { 600.0 };
                let baseline = [40.0_f32, 60.0, 30.0];
                let encode = |raw: [f32; 3]| {
                    let peak = raw.into_iter().fold(0.0_f32, f32::max);
                    raw.map(|v| {
                        if exposure == 0.0 {
                            v / peak.max(255.0)
                        } else {
                            v / peak * (1.0 - (-peak / exposure).exp())
                        }
                    })
                };
                let linear = |v: f32| {
                    if v <= 0.04045 {
                        v / 12.92
                    } else {
                        ((v + 0.055) / 1.055).powf(2.4)
                    }
                };
                let mut rgba = Vec::new();
                for flux in [1.0, 0.0, 0.5, 1.0, 0.0, 0.5] {
                    let rgb = encode(baseline.map(|v| v + sun * flux));
                    rgba.extend(rgb.map(|v| (v * 255.0).round() as u8));
                    rgba.push((flux * 255.0).round() as u8);
                }
                let data = crate::materials::TextureData {
                    label: "sun cache fixture".into(),
                    source: None,
                    width: 3,
                    height: 2,
                    rgba: (rgba).into(),
                    rgba16f: None,
                    mip_level_count: 1,
                    clamp: true,
                    srgb: true,
                };
                let original = crate::renderer::upload_texture(&device, &queue, &data);
                let mut cache = LightmapCache::new(&device, &queue, &[original], &[]);
                let encoding = crate::scene::BakedSunEncoding {
                    exposure,
                    gamma: 1.0,
                    compensate: 1.0,
                    brightness: 1.0,
                    deviance: 0.0,
                    samples: 1,
                };
                // Repeated full/zero/half/full updates must always read the
                // original. The non-multiple-of-eight dimensions test dispatch bounds.
                for strength in [1.0, 0.0, 0.5, 1.0] {
                    cache.bake(&device, &queue, encoding, [sun / 255.0; 3], strength);
                    let readback = device.create_buffer(&wgpu::BufferDescriptor {
                        label: None,
                        size: 512,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    });
                    let mut encoder = device.create_command_encoder(&Default::default());
                    encoder.copy_texture_to_buffer(
                        wgpu::TexelCopyTextureInfo {
                            texture: &cache.corrected[0]._texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::TexelCopyBufferInfo {
                            buffer: &readback,
                            layout: wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(256),
                                rows_per_image: Some(2),
                            },
                        },
                        wgpu::Extent3d {
                            width: 3,
                            height: 2,
                            depth_or_array_layers: 1,
                        },
                    );
                    queue.submit([encoder.finish()]);
                    readback
                        .slice(..)
                        .map_async(wgpu::MapMode::Read, |result| result.unwrap());
                    device
                        .poll(wgpu::PollType::Wait {
                            submission_index: None,
                            timeout: None,
                        })
                        .unwrap();
                    let bytes = readback.slice(..).get_mapped_range().unwrap();
                    for index in 0..6 {
                        let flux = data.rgba[index * 4 + 3] as f32 / 255.0;
                        let expected =
                            encode(baseline.map(|v| v + sun * flux * (1.0 - strength))).map(linear);
                        for channel in 0..3 {
                            let offset = index / 3 * 256 + index % 3 * 8 + channel * 2;
                            let bits = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
                            let exponent = (bits >> 10) & 31;
                            let mantissa = (bits & 1023) as f32 / 1024.0;
                            let actual = if exponent == 0 {
                                mantissa * 2.0_f32.powi(-14)
                            } else {
                                (1.0 + mantissa) * 2.0_f32.powi(exponent as i32 - 15)
                            };
                            assert!(
                                (actual - expected[channel]).abs() < 0.012,
                                "exposure {exposure}, strength {strength}, texel {index}, channel {channel}: {actual} vs {}",
                                expected[channel]
                            );
                        }
                    }
                }
            }
        });
    }
}

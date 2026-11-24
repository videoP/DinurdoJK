// Temporary JKA surface adapter for Blue Water's unmodified ocean field.
// This preserves promoted-water masks and existing Unified draws while the
// upstream surface, optics and reflection paths are integrated separately.
use blue_water_core::QualityTier;

const CONVERT: &str = r#"
@group(0) @binding(0) var displacement_in: texture_2d_array<f32>;
@group(0) @binding(1) var derivatives_in: texture_2d_array<f32>;
@group(0) @binding(2) var extras_in: texture_2d_array<f32>;
@group(0) @binding(3) var displacement_out: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(4) var normal_out: texture_storage_2d_array<rgba16float, write>;
@group(0) @binding(5) var<storage, read> crest_bounds: vec4<f32>;
@group(0) @binding(6) var<uniform> crest: vec4<f32>; // mode, FFT mix, analytic compression, unused
@compute @workgroup_size(8, 8, 1)
fn convert(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = vec2<i32>(id.xy);
    let layer = i32(id.z);
    let d = textureLoad(displacement_in, p, layer, 0);
    let slope = textureLoad(derivatives_in, p, layer, 0);
    let extra = textureLoad(extras_in, p, layer, 0);
    let horizontal = jka_crest_scale_from_bounds(crest_bounds, crest.y, crest.z, crest.x);
    // JKA's existing water vertex shader flips the sampled Z displacement;
    // its fragment shader flips the sampled Z slope. Preserve Blue Water's
    // +Z values after those existing operations.
    // Legacy displacement.w is unused (Godot writes zero; ordinary native
    // streak density is nonnegative). A negative value carries the constant
    // scale so analytic swell added by legacy consumers gets limited too.
    let tag = select(d.w, -horizontal, crest.x > 0.5);
    textureStore(displacement_out, p, layer, vec4<f32>(d.x * horizontal, d.y, -d.z * horizontal, tag));
    textureStore(normal_out, p, layer, vec4<f32>(slope.x, -slope.y, 0.0, extra.y));
}
"#;

// r_oceanUpdateSpread (live): 1 spreads each throttled Blue Water FFT update
// (r_oceanUpdates > 0) over the rendered frames of its period, as the original
// ocean path already does with its FFT phases; 0 encodes it whole on its tick.
static UPDATE_SPREAD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
pub fn update_spread() -> bool { UPDATE_SPREAD.load(std::sync::atomic::Ordering::Relaxed) }
pub fn set_update_spread(enabled: bool) { UPDATE_SPREAD.store(enabled, std::sync::atomic::Ordering::Relaxed); }

/// A Blue Water field update in flight (r_oceanUpdateSpread).
#[derive(Clone, Copy)]
struct FieldJob {
    /// Render time the job began; its slices follow `ocean_phases_due`.
    start: f64,
    /// Wave time the field is computed for: when it is expected to publish.
    time: f64,
    /// Transform passes encoded so far.
    done: usize,
}

pub struct BlueWaterOcean {
    pub field: blue_water_gpu::WaveField,
    pub surface_waves: blue_water_gpu::surface_waves::SurfaceWaves,
    pub shore_waves: [Option<blue_water_gpu::surface_waves::SurfaceWaves>; super::OCEAN_MAX_SURFACES],
    pub shore_maps: Option<std::sync::Arc<super::upstream_coast::Maps>>,
    shore_shelter: bool,
    pub original_surface: Option<super::original_surface::SurfaceRenderer>,
    /// Blue Water's ship wake (created while `OceanSettings::blue_wake` is on) and the commands waiting for a frame.
    pub wake: Option<super::blue_wake::BlueWake>,
    pub wake_pending: Vec<super::blue_wake::WakeCommand>,
    _displacement: wgpu::Texture,
    _normal: wgpu::Texture,
    pub displacement_view: wgpu::TextureView,
    pub normal_view: wgpu::TextureView,
    pipeline: wgpu::ComputePipeline,
    groups: [wgpu::BindGroup; 2],
    crest_params: wgpu::Buffer,
    breaker_mode: u8,
    time: f64,
    next_update: f64,
    last_step: f64,
    job: Option<FieldJob>,
    pub updated_this_frame: bool,
    pub(super) wave_revision: u64,
    pub adapter_enabled: bool,
}

impl BlueWaterOcean {
    pub fn waves(&self,slot:usize)->&blue_water_gpu::surface_waves::SurfaceWaves {
        self.shore_waves.get(slot).and_then(Option::as_ref).unwrap_or(&self.surface_waves)
    }

    pub fn set_shore(&mut self,device:&wgpu::Device,queue:&wgpu::Queue,maps:Option<std::sync::Arc<super::upstream_coast::Maps>>,shelter:bool) {
        if (self.shore_maps.as_ref().zip(maps.as_ref()).is_some_and(|(a,b)|std::sync::Arc::ptr_eq(a,b))
            || (self.shore_maps.is_none() && maps.is_none())) && self.shore_shelter == shelter {return;}
        self.shore_shelter=shelter;
        self.shore_waves=std::array::from_fn(|_|None);
        if let Some(maps)=&maps {
            for (slot,map) in maps.iter().filter(|(slot,_)|*slot<super::OCEAN_MAX_SURFACES) {
                let waves=blue_water_gpu::surface_waves::SurfaceWaves::with_shore_shelter(device,queue,&self.field,std::sync::Arc::clone(map),shelter);
                waves.write(queue,&self.field,self.time,1.0,1.0);
                self.shore_waves[*slot]=Some(waves);
            }
        }
        self.shore_maps=maps;
    }
    pub fn new(device: &wgpu::Device) -> Self {
        let mut field = blue_water_gpu::WaveField::new(device, QualityTier::High);
        let surface_waves = blue_water_gpu::surface_waves::SurfaceWaves::new(device, &field);
        field.prepare_composite_crest_limits(device, &surface_waves);
        let n = field.resolution();
        let create = |label| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: n,
                    height: n,
                    depth_or_array_layers: 3,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        let displacement = create("Blue Water JKA displacement adapter");
        let normal = create("Blue Water JKA normal adapter");
        let view = |texture: &wgpu::Texture| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        };
        let displacement_view = view(&displacement);
        let normal_view = view(&normal);
        let input = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        };
        let output = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format: wgpu::TextureFormat::Rgba16Float,
                view_dimension: wgpu::TextureViewDimension::D2Array,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Blue Water JKA adapter layout"),
            entries: &[input(0), input(1), input(2), output(3), output(4),
                wgpu::BindGroupLayoutEntry {
                    binding: 5, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false, min_binding_size: None }, count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false, min_binding_size: None }, count: None,
                }],
        });
        let crest_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Hybrid adapter crest control"), size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Blue Water JKA surface adapter"),
            source: wgpu::ShaderSource::Wgsl(format!("{}\n{}", blue_water_gpu::CREST_MATH_WGSL, CONVERT).into()),
        });
        let pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Blue Water JKA surface adapter"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Blue Water JKA surface adapter"),
            layout: Some(&pipe_layout),
            module: &module,
            entry_point: Some("convert"),
            compilation_options: Default::default(),
            cache: None,
        });
        let groups = std::array::from_fn(|set| {
            let source = field.views_for(set);
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Blue Water JKA surface adapter"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&source[0]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&source[1]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&source[2]),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(&displacement_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&normal_view),
                    },
                    wgpu::BindGroupEntry { binding: 5, resource: field.crest_limits_buffer().as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 6, resource: crest_params.as_entire_binding() },
                ],
            })
        });
        Self {
            field,
            surface_waves,
            shore_waves: std::array::from_fn(|_|None),
            shore_maps: None,
            shore_shelter: true,
            original_surface: None,
            wake: None,
            wake_pending: Vec::new(),
            _displacement: displacement,
            _normal: normal,
            displacement_view,
            normal_view,
            pipeline,
            groups,
            crest_params,
            breaker_mode: 0,
            time: 0.0,
            next_update: 0.0,
            last_step: 0.0,
            updated_this_frame: false,
            wave_revision: 0,
            adapter_enabled: true,
            job: None,
        }
    }

    /// A map ocean's physics waves (jaPRO `BG_SailingSea`) replace the FFT's
    /// two long tiles, so the drawn long and medium waves are the ones physics
    /// floats in; None restores Blue Water's own spectrum.
    pub fn set_sailing_sea(&mut self, sea: Option<&jka_movement::SailingSea>) {
        let lattice = sea.map(|sea| std::sync::Arc::new(sea.waves.iter().map(|w| blue_water_gpu::LatticeWave {
            cascade: w.cascade, nx: w.nx, nz: w.nz, re: w.re, im: w.im, m: w.m,
        }).collect::<Vec<_>>()));
        self.field.set_authored_lattice(lattice);
    }

    pub fn set_time(&mut self, time: f32) {
        self.time = time as f64;
        self.next_update = self.time;
        self.last_step = self.time;
        self.job = None;
    }

    pub fn encode(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        delta: f32,
        updates_per_second: f32,
        waves: super::BlueWaterWaves,
        swell: [[f32;4];2],
        breaker_mode: u8,
        wave_counts: [u32; 2],
        detail_gains: [f32; 2],
    ) {
        self.updated_this_frame = false;
        // r_oceanLongWaves / r_oceanMediumWaves and r_oceanMediumWaveDetail /
        // r_oceanSmallWaveDetail; each rebuilds the spectrum on change.
        self.field.set_long_waves(wave_counts[0], wave_counts[1]);
        self.field.set_wave_detail_gains(detail_gains[0], detail_gains[1]);
        if self.breaker_mode != breaker_mode {
            self.breaker_mode = breaker_mode;
            self.field.request_update();
            self.next_update = self.time;
        }
        self.field.set_crest_limits(breaker_mode > 0);
        let control = [f32::from(breaker_mode), swell[1][2], (swell[0][3] * swell[0][2] * swell[1][1]).abs(), 0.0];
        queue.write_buffer(&self.crest_params, 0, bytemuck::bytes_of(&control));
        let delta = f64::from(delta.max(0.0));
        self.time += delta;
        let period = if updates_per_second > 0.0 {
            1.0 / f64::from(updates_per_second)
        } else {
            0.0
        };
        // An update in flight: the passes now due, then publish with its last.
        if let Some(mut job) = self.job.take() {
            let total = self.field.transform_pass_count();
            // The finish (resolve, mips, crest limits) is the final phase.
            let phases = total + 1;
            let due = super::ocean_phases_due((self.time - job.start) as f32, 0.0, period as f32, phases);
            let transforms = due.min(total);
            if transforms > job.done {
                self.field.encode_transform_passes(encoder, job.done..transforms);
                job.done = transforms;
            }
            if due < phases {
                self.job = Some(job);
                return;
            }
            self.field.finish_update(encoder);
            self.publish(queue, encoder, job.time, swell);
        }
        if period > 0.0 && self.time < self.next_update {
            return;
        }
        self.next_update = if period > 0.0 {
            self.next_update
                + ((self.time - self.next_update) / period).floor().max(0.0) * period
                + period
        } else {
            self.time
        };
        let spread = period > 0.0 && update_spread();
        // A spread update publishes most of a period later: compute the field
        // for that time, so the shown waves (and BG_SailingSea physics, which
        // reads the same clock) are no staler than a whole update on its tick.
        let phases = self.field.transform_pass_count() + 1;
        let time = if spread { self.time + period * (phases - 1) as f64 / phases as f64 } else { self.time };
        let step_dt = (time - self.last_step).max(0.0);
        self.last_step = time;
        let params = waves.parameters();
        if spread {
            if self.field.begin_update(queue, encoder, params, false, time, step_dt, 1.0, 1.0) {
                // The first phase now; the rest as the period elapses.
                self.field.encode_transform_passes(encoder, 0..1);
                self.job = Some(FieldJob { start: self.time, time, done: 1 });
            }
        } else if self.field.update(queue, encoder, params, false, time, step_dt, 1.0, 1.0) {
            self.publish(queue, encoder, time, swell);
        }
    }

    /// A finished field update: CPU wave copies and the JKA surface textures.
    fn publish(&mut self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, time: f64, swell: [[f32; 4]; 2]) {
        self.updated_this_frame = true;
        self.wave_revision = self.wave_revision.wrapping_add(1);
        self.surface_waves.write(queue, &self.field, time, 1.0, 1.0);
        for waves in self.shore_waves.iter().flatten() {waves.write_with_swell(queue,&self.field,time,1.0,1.0,swell);}
        if !self.adapter_enabled { return; }
        let n = self.field.resolution();
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Blue Water JKA surface conversion"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.groups[self.field.current_index()], &[]);
        pass.dispatch_workgroups(n.div_ceil(8), n.div_ceil(8), 3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blue_water_jka_texture_conversion_dispatches() {
        futures_lite::future::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&Default::default())
                .await
                .expect("GPU adapter");
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_limits: adapter.limits(),
                    ..Default::default()
                })
                .await
                .expect("GPU device");
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let mut ocean = BlueWaterOcean::new(&device);
            let mut encoder = device.create_command_encoder(&Default::default());
            // 0 Hz: one whole update (a throttled one may spread over frames).
            ocean.encode(&queue, &mut encoder, 1.0 / 48.0, 0.0, super::super::BlueWaterWaves::default(),[[0.0;4],[0.0,0.0,1.0,0.0]], 0, [0, 0], [1.0, 1.0]);
            let n = ocean.field.resolution();
            let read = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Blue Water adapter readback"),
                size: u64::from(n) * u64::from(n) * 8,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &ocean._displacement,
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
                .expect("Blue Water adapter submit");
            assert!(
                scope.pop().await.is_none(),
                "Blue Water adapter validation error"
            );
            let bytes = read
                .slice(..)
                .get_mapped_range()
                .expect("Blue Water adapter map");
            assert!(
                bytes.iter().any(|&byte| byte != 0),
                "Blue Water JKA displacement adapter is empty"
            );
        });
    }
}

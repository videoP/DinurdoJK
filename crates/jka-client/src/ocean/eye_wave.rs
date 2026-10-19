//! Height of the ocean surface straight above/below the camera, solved on the GPU.
//!
//! Hybrid draws two different surfaces: the original Blue Water mesh above water and
//! the Godot mesh (fed by an adapter copy of the same FFT) below it. They do NOT
//! agree: upstream `displacement()` reads its two long cascades through stochastic
//! tiling, the adapter reads them plainly. A vertex is drawn at `grid +
//! displacement(grid)`, so the surface over an XZ is found by inverting that
//! horizontal displacement. Two one-thread compute passes do that solve:
//!
//! * **native**: the preserved upstream `displacement()` itself (blue-water-gpu's
//!   `eye_height.wgsl`), i.e. exactly the surface the Blue Water mesh rasterizes;
//! * **adapter**: the Godot vertex shader's lookup, plus the two-cascade value the
//!   tint shader reads at the eye (`naive`).
//!
//! The CPU reads both a few frames later through one 32-byte readback. Native is the
//! single source of truth for "is the eye under water" (tint, cull, mesh choice);
//! the tint shader is corrected towards it at the eye and the Godot mesh is lifted
//! onto it near the camera, so every system agrees at the waterline.
use bytemuck::{Pod, Zeroable};

const SHADER: &str = r#"
struct Params {
    map_scales: array<vec4<f32>, 3>,
    ocean_info: vec4<f32>,
    swell: vec4<f32>,
    swell_motion: vec4<f32>,
    eye: vec4<f32>,
};
@group(0) @binding(0) var displacement: texture_2d_array<f32>;
@group(0) @binding(1) var wave_sampler: sampler;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var<storage, read_write> result: array<vec4<f32>, 2>;

// Mirrors sample_ocean_displacement() in bsp.wgsl for the live (nearest) mesh
// level: every cascade, the Z flip, the horizontal scale and the swell term.
fn displacement_m(world_m: vec2<f32>) -> vec3<f32> {
    var d = vec3<f32>(0.0);
    if params.map_scales[0].z > 0.0 {
    for (var i = 0u; i < 3u; i = i + 1u) {
        let scale = params.map_scales[i];
        let uv = world_m * vec2<f32>(1.0, -1.0) * scale.xy;
        d += textureSampleLevel(displacement, wave_sampler, uv, i32(i), 0.0).xyz
            * vec3<f32>(1.0, 1.0, -1.0) * scale.z;
    }
    }
    d.x *= params.swell_motion.z;
    d.z *= params.swell_motion.z;
    let phase = dot(world_m, params.swell.xy) * params.swell.z - params.swell_motion.x * params.swell_motion.w;
    let horizontal = params.swell.w * params.swell_motion.y * cos(phase);
    d += vec3<f32>(params.swell.x * horizontal, params.swell.w * sin(phase), params.swell.y * horizontal);
    return d;
}

@compute @workgroup_size(1)
fn main() {
    let units_per_metre = max(params.ocean_info.z, 0.001);
    let target_m = params.eye.xy / units_per_metre;
    // Find the undisplaced point q whose displaced position is over the eye. Damped:
    // steep, choppy waves make the plain fixed point diverge.
    var q = target_m;
    var d = displacement_m(q);
    for (var i = 0u; i < 12u; i = i + 1u) {
        q = q + 0.6 * ((target_m - d.xz) - q);
        d = displacement_m(q);
    }
    let residual_m = length(q + d.xz - target_m);
    // What ocean_optics.wgsl's jka_wave_height() reads here without the eye term:
    // the two large cascades at the un-inverted position.
    var naive_m = 0.0;
    if params.map_scales[0].z > 0.0 {
    for (var i = 0u; i < 2u; i = i + 1u) {
        let scale = params.map_scales[i];
        let uv = target_m * vec2<f32>(1.0, -1.0) * scale.xy;
        naive_m += textureSampleLevel(displacement, wave_sampler, uv, i32(i), 0.0).y * scale.z;
    }
    }
    let phase = dot(target_m, params.swell.xy) * params.swell.z - params.swell_motion.x * params.swell_motion.w;
    naive_m += params.swell.w * sin(phase);
    result[0] = vec4<f32>(d.y * units_per_metre, residual_m * units_per_metre, 1.0, naive_m * units_per_metre);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct EyeWaveParams {
    pub map_scales: [[f32; 4]; 3],
    pub ocean_info: [f32; 4],
    pub swell: [f32; 4],
    pub swell_motion: [f32; 4],
    pub eye: [f32; 4],
}

/// Native probe parameters: eye xz in upstream metres, mesh spacing in metres.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct NativeEyeParams {
    pub eye: [f32; 4],
    pub swell: [[f32; 4]; 2],
}

/// Everything the native solve needs from the Blue Water ocean.
pub struct NativeProbeInput<'a> {
    pub swell: [[f32; 4]; 2],
    pub waves_group: &'a wgpu::BindGroup,
    pub eye_xz_metres: [f32; 2],
    /// Grid spacing the mesh uses at the eye, metres (the finest ring).
    pub spacing_metres: f32,
}

/// Largest acceptable fixed-point residual, in JKA units. A solve that did not
/// converge (overturning crest) is dropped rather than trusted.
const MAX_RESIDUAL_UNITS: f32 = 8.0;

/// One frame of the Hybrid classification, kept for the `hybridwave` dump command.
#[derive(Clone, Copy)]
pub struct EyeWaveRecord {
    pub seconds: f64,
    pub eye: [f32; 3],
    pub yaw_pitch: [f32; 2],
    pub slot: Option<usize>,
    pub plane: f32,
    pub probe_valid: bool,
    pub probe_offset: f32,
    pub probe_correction: f32,
    pub adapter_offset: f32,
    pub native_valid: bool,
    pub camera_water_surface: f32,
    pub submerged: bool,
    pub godot_mesh: bool,
    pub cull_radius: Option<f32>,
}

/// Last few seconds of [`EyeWaveRecord`]s.
#[derive(Default)]
pub struct EyeWaveLog {
    ring: std::collections::VecDeque<EyeWaveRecord>,
}

impl EyeWaveLog {
    const CAPACITY: usize = 600;

    pub fn push(&mut self, record: EyeWaveRecord) {
        if self.ring.len() == Self::CAPACITY {
            self.ring.pop_front();
        }
        self.ring.push_back(record);
    }

    /// Header text, then one CSV row per recorded frame (oldest first).
    pub fn write(&self, path: &std::path::Path, header: &str) -> std::io::Result<()> {
        use std::fmt::Write as _;
        let mut text = String::from(header);
        text.push_str("\n\nframes (oldest first); probe_offset = surface height over the eye above the plane (native when native_valid, else adapter); eye_minus_surface = eye_y - (plane + probe_offset)\n");
        text.push_str("t_s,eye_x,eye_y,eye_z,yaw,pitch,slot,plane,eye_minus_plane,probe_valid,native_valid,probe_offset,adapter_offset,probe_correction,eye_minus_surface,camera_water_surface,cpu_submerged,mesh,cull_radius\n");
        for r in &self.ring {
            let _ = writeln!(
                text,
                "{:.3},{:.2},{:.2},{:.2},{:.1},{:.1},{},{:.2},{:.2},{},{},{:.2},{:.2},{:.2},{:.2},{:.2},{},{},{}",
                r.seconds, r.eye[0], r.eye[1], r.eye[2], r.yaw_pitch[0].to_degrees(), r.yaw_pitch[1].to_degrees(),
                r.slot.map_or(-1, |s| s as i64), r.plane, r.eye[1] - r.plane, r.probe_valid, r.native_valid,
                r.probe_offset, r.adapter_offset, r.probe_correction, r.eye[1] - (r.plane + r.probe_offset),
                r.camera_water_surface, r.submerged, if r.godot_mesh { "Godot" } else { "BlueWater" },
                r.cull_radius.map_or(-1.0, |v| v),
            );
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, text)
    }
}

enum Readback {
    Idle,
    Copied,
    Mapping(std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyeWaveSample {
    pub slot: usize,
    /// Godot-mesh surface height over the eye above the pool plane, JKA units.
    pub adapter: f32,
    /// What the tint shader's two-cascade lookup reads at the eye, same units.
    pub naive: f32,
    /// The surface the original mesh rasterizes over the eye, same units.
    pub native: Option<f32>,
}

impl EyeWaveSample {
    /// The authoritative surface height above the plane (JKA units).
    pub fn offset(&self) -> f32 {
        self.native.unwrap_or(self.adapter)
    }

    /// Added to the tint shader's two-cascade lookup near the eye.
    pub fn correction(&self) -> f32 {
        self.offset() - self.naive
    }

    /// Vertical lift that puts the Godot mesh on the authoritative surface at the eye.
    pub fn mesh_shift(&self) -> Option<f32> {
        self.native.map(|native| native - self.adapter)
    }
}

struct NativePipeline {
    pipeline: wgpu::ComputePipeline,
    group: wgpu::BindGroup,
    empty_group: wgpu::BindGroup,
    params: wgpu::Buffer,
}

pub struct EyeWaveProbe {
    pipeline: wgpu::ComputePipeline,
    group: wgpu::BindGroup,
    params: wgpu::Buffer,
    result: wgpu::Buffer,
    readback: wgpu::Buffer,
    native: Option<NativePipeline>,
    state: Readback,
    /// Pool the in-flight copy was dispatched for.
    in_flight_slot: usize,
    latest: Option<EyeWaveSample>,
}

fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: false },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
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

impl EyeWaveProbe {
    pub fn new(
        device: &wgpu::Device,
        displacement: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
        waves_layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ocean eye wave probe layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                uniform_entry(2),
                storage_entry(3),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ocean eye wave probe"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Ocean eye wave probe"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Ocean eye wave probe"),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ocean eye wave probe parameters"),
            size: std::mem::size_of::<EyeWaveParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let result = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ocean eye wave probe result"),
            size: 32,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ocean eye wave probe readback"),
            size: 32,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Ocean eye wave probe"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(displacement) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: result.as_entire_binding() },
            ],
        });
        let native = Self::native_pipeline(device, waves_layout, &result);
        Self {
            pipeline,
            group,
            params,
            result,
            readback,
            native,
            state: Readback::Idle,
            in_flight_slot: 0,
            latest: None,
        }
    }

    /// The upstream `displacement()` compute probe. `None` (with a log line) if the
    /// composed shader does not build; the adapter probe still works.
    fn native_pipeline(
        device: &wgpu::Device,
        waves_layout: &wgpu::BindGroupLayout,
        result: &wgpu::Buffer,
    ) -> Option<NativePipeline> {
        let mut composer = match blue_water_gpu::shader_composer::composer() {
            Ok(composer) => composer,
            Err(error) => {
                eprintln!("[EYE WAVE] native probe unavailable (composer): {error:?}");
                return None;
            }
        };
        let module = match blue_water_gpu::shader_composer::eye_height_module(&mut composer) {
            Ok(module) => module,
            Err(error) => {
                eprintln!("[EYE WAVE] native probe unavailable: {}", error.emit_to_string(&composer));
                return None;
            }
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Native eye height probe layout"),
            entries: &[uniform_entry(0), storage_entry(1)],
        });
        let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Native eye height probe empty layout"),
            entries: &[],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Native eye height probe"),
            source: wgpu::ShaderSource::Naga(std::borrow::Cow::Owned(module)),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Native eye height probe"),
            bind_group_layouts: &[Some(&layout), Some(&empty_layout), Some(waves_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("Native eye height probe"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Native eye height probe parameters"),
            size: std::mem::size_of::<NativeEyeParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Native eye height probe"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: result.as_entire_binding() },
            ],
        });
        let empty_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Native eye height probe empty"),
            layout: &empty_layout,
            entries: &[],
        });
        Some(NativePipeline { pipeline, group, empty_group, params })
    }

    pub fn has_native(&self) -> bool {
        self.native.is_some()
    }

    /// Forgets the last sample (camera left the pool or the mode no longer needs it).
    pub fn clear(&mut self) {
        self.latest = None;
    }

    pub fn latest(&self, slot: usize) -> Option<EyeWaveSample> {
        self.latest.filter(|sample| sample.slot == slot)
    }

    /// Collects a finished readback, dispatches this frame's solves and, when no
    /// copy is outstanding, queues the next readback. No CPU wait anywhere.
    pub fn encode(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        params: &EyeWaveParams,
        native: Option<NativeProbeInput>,
        slot: usize,
    ) {
        if matches!(self.state, Readback::Copied) {
            let (send, receive) = std::sync::mpsc::channel();
            self.readback.slice(..).map_async(wgpu::MapMode::Read, move |result| {
                let _ = send.send(result);
            });
            self.state = Readback::Mapping(receive);
        }
        if let Readback::Mapping(receive) = &self.state {
            if let Ok(status) = receive.try_recv() {
                if status.is_ok() {
                    if let Ok(view) = self.readback.slice(..).get_mapped_range() {
                        let value = |i: usize| f32::from_le_bytes(view[i * 4..i * 4 + 4].try_into().unwrap());
                        let (adapter, residual, valid, naive) = (value(0), value(1), value(2), value(3));
                        // The native solve works in upstream metres.
                        let metres = super::OCEAN_UNITS_PER_METER;
                        let (native, native_residual, native_valid) =
                            (value(4) * metres, value(5) * metres, value(6));
                        let ok = |v: f32| v.is_finite();
                        // A failed solve keeps the previous sample (held, not zeroed to the flat plane).
                        if valid > 0.5 && ok(adapter) && ok(naive) && ok(residual) && residual <= MAX_RESIDUAL_UNITS {
                            let native = (native_valid > 0.5 && ok(native) && ok(native_residual)
                                && native_residual <= MAX_RESIDUAL_UNITS)
                                .then_some(native);
                            self.latest = Some(EyeWaveSample { slot: self.in_flight_slot, adapter, naive, native });
                        }
                    }
                    self.readback.unmap();
                }
                self.state = Readback::Idle;
            }
        }
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(params));
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Ocean eye wave probe"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        // Without a native sample the previous native value must not be reused.
        let mut native_ran = false;
        if let (Some(native_pipeline), Some(input)) = (self.native.as_ref(), native) {
            let eye = NativeEyeParams {
                eye: [input.eye_xz_metres[0], input.eye_xz_metres[1], input.spacing_metres, 0.0],
                swell: input.swell,
            };
            queue.write_buffer(&native_pipeline.params, 0, bytemuck::bytes_of(&eye));
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Native eye height probe"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&native_pipeline.pipeline);
            pass.set_bind_group(0, &native_pipeline.group, &[]);
            pass.set_bind_group(1, &native_pipeline.empty_group, &[]);
            pass.set_bind_group(2, input.waves_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
            native_ran = true;
        }
        if matches!(self.state, Readback::Idle) {
            encoder.copy_buffer_to_buffer(&self.result, 0, &self.readback, 0, 32);
            self.in_flight_slot = slot;
            self.state = Readback::Copied;
        }
        if !native_ran {
            // Next dispatch of the native probe overwrites result[1]; until then a
            // stale value could be copied, so invalidate it from the CPU side.
            self.invalidate_native_slot(queue);
        }
    }

    fn invalidate_native_slot(&self, queue: &wgpu::Queue) {
        queue.write_buffer(&self.result, 16, bytemuck::bytes_of(&[0.0f32; 4]));
    }
}

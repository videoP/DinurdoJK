//! Fire_Smoke_Explosive GPU sprite particles, simulated on the GPU as in UE
//! (`embers.wgsl`, constants and module stacks from the assets'
//! LastHlslTranslationGPU) and drawn straight from the simulation buffer
//! (`ember_vertex`). This module owns the per-source pools, the tick
//! dispatches and the brush upload. Two pool kinds:
//! - Turret embers (`r_ninjaFireEmbers`): the two visible emitters of
//!   `NS_WriteDataCh_GPU_Turret` (VisualizedParticles1, bright
//!   velocity-aligned streaks, and VisualizedParticles2, soft dots that fall
//!   and settle) on every Ninja fire source. The hidden third emitter only
//!   feeds the turret's flame simulation and is not ported.
//! - Burning Man sparks: NS_Sparks_DecorationOnly2 (NS_PASSIVE_Sparks), the
//!   pawn's decoration emitter.
//!
//! DinurdoJK substitutions for the turret embers (also in
//! DinurdoJK-ninja/UPSTREAM.md, "Turret embers"):
//! - Scale. The stack is authored around the turret's 25 m Ninja field. Each
//!   source runs it in a local frame (centimetres, Z up) and maps it
//!   to the world with one length factor `s` = source field height / 25 m
//!   (times the authored-EFX magnification of a promoted torch). Positions,
//!   velocities and accelerations scale by `s`; times do not. Spawn rates scale
//!   by `s^2`, keeping the settled-ember density per floor area. Sprites are
//!   drawn at `sqrt(s)` of their authored size so the soft dots stay visible
//!   at JKA fire sizes; mass and collision radius keep the authored size.
//! - Flame sources use a sparse spray and random launch elevations.
//! - Collision. GPU Distance Fields become the solid BSP brushes around the
//!   source (sphere radius |SpriteSize|/2, as CalculateRadiusFromSpriteSize).
//!   The response keeps restitution 0.2, no bounce friction, the force reset
//!   on contact, and rest on floor-like normals. UE's penetration rest
//!   counter is replaced by a normal-speed threshold.
//! - The VP2 alpha curve is sampled piecewise-linearly between its keys.
//! - Ambient wind enters as AdvectionVelocity, as for the Burning Man sparks
//!   (authored zero; promoted fires already pass no wind).
//! - EffectType InputForFluidSim_DistanceCull_Asleep_35m: a source farther
//!   than 35 m neither simulates nor draws.
use std::collections::VecDeque;

use glam::Vec3;
use jka_river::river_ninja::checked_scope;

use super::CM_PER_MAP_UNIT;
use super::brush_collision::{BrushBuffer, pack_brushes};
use crate::ninja_fire_debug::WorldBrush;

/// TurretFlameThrower ExtentsXYZ.
const TURRET_FIELD_CM: f32 = 2500.0;
const ASLEEP_DISTANCE_CM: f32 = 3500.0;
const PARTICLE_BYTES: u64 = 80;
const PARAMS_BYTES: u64 = 112;
const ARGS_BYTES: u64 = 40;
const DRAW_ARGS: u64 = 24;
const MAX_CAPACITY: u32 = 32_768;
/// Up to this many ticks per frame, each at most 1/60 s.
const MAX_TICKS: u32 = 4;
/// Uniform slots (dynamic offsets) for the ticks of one frame.
const PARAMS_STRIDE: u64 = 256;
/// Local collision neighbourhood around the emitter, in authored cm: the
/// jet rises about 6 m and its embers fall for up to 3 s.
const REACH_XY: f32 = 2500.0;
const REACH_UP: f32 = 1500.0;
const REACH_DOWN: f32 = 6000.0;
/// Anchor history kept for a moving source: the longest ember lifetime.
const HISTORY_SECONDS: f32 = 3.0;
/// Local TilingCurl64 source for the Burning Man sparks.
pub(super) const SPARK_CURL_QPATH: &str = "textures/japro/fire/tiling_curl64.f32x3";
pub(super) const SPARK_CURL_BYTES: usize = 64 * 64 * 64 * 12;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PoolKind {
    TurretEmbers,
    BurningManSparks,
}

impl PoolKind {
    /// (shader kind, spawn rate per second, longest lifetime) per spawn slot;
    /// the rest of each recipe is in ember_common.wgsl.
    fn slots(self) -> [(u32, f32, f32); 2] {
        match self {
            Self::TurretEmbers => [(0, 80.0, 2.0), (1, 180.0, 3.0)],
            // Level override of User.SpawnRate: 50/s.
            Self::BurningManSparks => [(2, 50.0, 1.5), (2, 0.0, 0.0)],
        }
    }

    /// CurlNoiseForce: Random Seed, Noise Frequency, Pan Noise Field.
    fn curl(self) -> (u32, f32, Vec3) {
        match self {
            Self::TurretEmbers => (13, 4.0, Vec3::new(0.1, 0.0, 0.0)),
            Self::BurningManSparks => (11, 8.0, Vec3::new(2.0, 1.0, 1.0)),
        }
    }
}

/// Length factor for a fire preset's embers, or None for smoke-only presets.
pub(super) fn ember_scale(preset: jka_ninja::FlamePreset, desc: &jka_ninja::FlameDesc) -> Option<f32> {
    use jka_ninja::FlamePreset::*;
    matches!(preset, Bonfire | BurningMan | Candle | Torch | Logfire | BurningMan3)
        .then(|| desc.extent_cm[1] / TURRET_FIELD_CM)
}

/// Raw little-endian f32 xyz voxels (x fastest) as RGBA16F, the format UE
/// keeps static vector fields in on the GPU.
pub(super) fn curl_rgba16f(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if bytes.len() % 12 != 0 {
        return Err("curl field is not f32 xyz".to_owned());
    }
    let mut out = Vec::with_capacity(bytes.len() / 12 * 8);
    for xyz in bytes.chunks_exact(12) {
        for lane in 0..4 {
            let value = if lane < 3 {
                f32::from_le_bytes(xyz[lane * 4..lane * 4 + 4].try_into().expect("float lane"))
            } else {
                0.0
            };
            if !value.is_finite() {
                return Err("curl field contains nonfinite values".to_owned());
            }
            out.extend_from_slice(&jka_river::river_slw::f32_to_f16(value).to_le_bytes());
        }
    }
    Ok(out)
}

/// A tiling RGBA16F vector field, (-500,-500,0)-(500,500,1000) cm.
pub(super) fn curl_texture(device: &wgpu::Device, queue: &wgpu::Queue, label: &str, size: u32, rgba16f: &[u8])
    -> Result<wgpu::TextureView, String>
{
    if rgba16f.len() != (size as usize).pow(3) * 8 {
        return Err(format!("{label} expected {} bytes, got {}", (size as usize).pow(3) * 8, rgba16f.len()));
    }
    let extent = wgpu::Extent3d { width: size, height: size, depth_or_array_layers: size };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        rgba16f,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size * 8), rows_per_image: Some(size) },
        extent,
    );
    Ok(texture.create_view(&Default::default()))
}

fn entry(binding: u32, visibility: wgpu::ShaderStages, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None }
}

fn storage(read_only: bool) -> wgpu::BindingType {
    wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None,
    }
}

fn uniform(dynamic: bool) -> wgpu::BindingType {
    wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: dynamic, min_binding_size: None,
    }
}

/// Simulation pipelines and the turret's TilingCurl32 field, shared by every pool.
pub(super) struct EmberCompute {
    sim_layout: wgpu::BindGroupLayout,
    args_layout: wgpu::BindGroupLayout,
    pub(super) draw_layout: wgpu::BindGroupLayout,
    begin: wgpu::ComputePipeline,
    update: wgpu::ComputePipeline,
    spawn: wgpu::ComputePipeline,
    end: wgpu::ComputePipeline,
    /// Emitter.VectorField32 = TilingCurl32 (turret embers); None when not in this build.
    pub(super) curl32: Option<wgpu::TextureView>,
    wrap: wgpu::Sampler,
}

impl EmberCompute {
    pub(super) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Self, String> {
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let c = wgpu::ShaderStages::COMPUTE;
        let sim_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ninja fire particle simulation"),
            entries: &[
                entry(0, c, uniform(true)), entry(1, c, storage(true)), entry(2, c, storage(false)),
                entry(3, c, storage(false)), entry(4, c, storage(true)),
                entry(5, c, wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D3,
                    multisampled: false,
                }),
                entry(6, c, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
            ],
        });
        let args_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ninja fire particle arguments"),
            entries: &[entry(0, c, storage(false))],
        });
        let v = wgpu::ShaderStages::VERTEX;
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ninja fire particle sprites"),
            entries: &[entry(0, v, storage(true)), entry(1, v, uniform(false))],
        });
        let sim_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Ninja fire particle simulation"),
            bind_group_layouts: &[Some(&sim_layout)],
            immediate_size: 0,
        });
        let args_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Ninja fire particle bookkeeping"),
            bind_group_layouts: &[Some(&sim_layout), Some(&args_layout)],
            immediate_size: 0,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ninja fire particle simulation"),
            source: wgpu::ShaderSource::Wgsl(
                format!("{}\n{}", include_str!("ember_common.wgsl"), include_str!("embers.wgsl")).into(),
            ),
        });
        let make = |entry: &str, layout: &wgpu::PipelineLayout| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let begin = make("em_begin", &args_pipeline_layout);
        let update = make("em_update_main", &sim_pipeline_layout);
        let spawn = make("em_spawn_main", &sim_pipeline_layout);
        let end = make("em_end", &args_pipeline_layout);
        // TilingCurl32 source data: 32^3 RGBA16F voxels, x fastest.
        let curl32 = jka_ninja::coast_whitewater_curl_field()
            .map(|bytes| curl_texture(device, queue, "Ninja turret ember TilingCurl32", 32, bytes))
            .transpose()?;
        let wrap = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Ninja fire particle curl wrap"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let memory = checked_scope(memory);
        let validation = checked_scope(validation);
        validation.and(memory)?;
        Ok(Self { sim_layout, args_layout, draw_layout, begin, update, spawn, end, curl32, wrap })
    }
}

/// One pool's frame inputs.
pub(super) struct EmberFrame<'a> {
    /// Spawn point, render axes, map units.
    pub anchor: Vec3,
    pub camera: Vec3,
    /// 0..1 emission strength (body fuel; 1 for placed fires).
    pub fuel: f32,
    pub time: f32,
    pub dt: f32,
    pub wind_cm_s: [f32; 3],
    /// Collision brushes (turret embers only).
    pub brushes: Option<&'a [WorldBrush]>,
}

/// One source's particle pool: ping-pong storage, counters, indirect arguments.
pub(super) struct ParticlePool {
    kind: PoolKind,
    /// Render-axis centimetres of the local origin; fixed for the pool's lifetime.
    frame_cm: Vec3,
    scale: f32,
    seed: u32,
    capacity: u32,
    curl: wgpu::TextureView,
    buffers: [wgpu::Buffer; 2],
    counters: wgpu::Buffer,
    args: wgpu::Buffer,
    params: wgpu::Buffer,
    brushes: BrushBuffer,
    brush_count: u32,
    sim_groups: [wgpu::BindGroup; 2],
    args_group: wgpu::BindGroup,
    draw_groups: [wgpu::BindGroup; 2],
    /// Buffer the last tick wrote (what the sprites draw).
    current: usize,
    ticked: bool,
    awake: bool,
    previous_anchor: Option<Vec3>,
    /// Recent local anchors (time, position) for the collision neighbourhood.
    history: VecDeque<(f32, Vec3)>,
    gathered: Option<(usize, Vec3, Vec3)>,
    ticks: u32,
}

impl ParticlePool {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        device: &wgpu::Device, queue: &wgpu::Queue, compute: &EmberCompute, curl: &wgpu::TextureView,
        kind: PoolKind, anchor: Vec3, scale: f32, seed: u32,
    ) -> Result<Self, String> {
        if !(scale.is_finite() && scale > 0.0) {
            return Err("invalid particle scale".to_owned());
        }
        let capacity = kind.slots().iter()
            .filter(|&&(_, rate, _)| rate > 0.0)
            .map(|&(_, rate, lifetime)| (rate * scale * scale * lifetime).ceil() as u32 + 16)
            .sum::<u32>()
            .min(MAX_CAPACITY);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let make = |label, size, usage| device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label), size, usage, mapped_at_creation: false,
        });
        let storage = wgpu::BufferUsages::STORAGE;
        let size = u64::from(capacity) * PARTICLE_BYTES;
        let buffers = [
            make("Ninja fire particles A", size, storage),
            make("Ninja fire particles B", size, storage),
        ];
        let counters = make("Ninja fire particle counters", 32, storage | wgpu::BufferUsages::COPY_DST);
        let args = make("Ninja fire particle indirect", ARGS_BYTES,
            storage | wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST);
        let params = make("Ninja fire particle params", PARAMS_STRIDE * u64::from(MAX_TICKS),
            wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
        let draw = make("Ninja fire particle draw", 32, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
        let brushes = BrushBuffer::new(device, "Ninja fire particle brushes");
        queue.write_buffer(&counters, 0, &[0u8; 32]);
        queue.write_buffer(&args, 0, &[0u8; ARGS_BYTES as usize]);
        let frame_cm = anchor * CM_PER_MAP_UNIT;
        let draw_values: [f32; 8] = [
            frame_cm.x, frame_cm.y, frame_cm.z, scale,
            scale.sqrt() / CM_PER_MAP_UNIT, CM_PER_MAP_UNIT, 0.0, 0.0,
        ];
        queue.write_buffer(&draw, 0, bytemuck::cast_slice(&draw_values));
        let draw_group = |buffer: &wgpu::Buffer| device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Ninja fire particle sprites"),
            layout: &compute.draw_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: draw.as_entire_binding() },
            ],
        });
        let draw_groups = [draw_group(&buffers[0]), draw_group(&buffers[1])];
        let args_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Ninja fire particle arguments"),
            layout: &compute.args_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: args.as_entire_binding() }],
        });
        let sim_groups = sim_groups(device, compute, curl, &params, &buffers, &counters, &brushes.buffer);
        let memory = checked_scope(memory);
        let validation = checked_scope(validation);
        validation.and(memory)?;
        Ok(Self {
            kind, frame_cm, scale, seed, capacity, curl: curl.clone(), buffers, counters, args, params, brushes,
            brush_count: 0, sim_groups, args_group, draw_groups, current: 0, ticked: false, awake: false,
            previous_anchor: None, history: VecDeque::new(), gathered: None, ticks: 0,
        })
    }

    pub(super) fn scale(&self) -> f32 {
        self.scale
    }

    pub(super) fn encode(
        &mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder,
        compute: &EmberCompute, frame: &EmberFrame<'_>,
    ) {
        self.awake = self.kind != PoolKind::TurretEmbers
            || frame.anchor.distance(frame.camera) * CM_PER_MAP_UNIT <= ASLEEP_DISTANCE_CM;
        if !self.awake {
            self.previous_anchor = None;
            return;
        }
        let anchor = self.to_local(frame.anchor * CM_PER_MAP_UNIT);
        let previous = self.previous_anchor.replace(anchor).unwrap_or(anchor);
        self.history.push_back((frame.time, anchor));
        while self.history.front().is_some_and(|&(time, _)| frame.time - time > HISTORY_SECONDS) {
            self.history.pop_front();
        }
        self.update_brushes(device, compute, queue, frame.brushes);
        if !(frame.dt > 0.0) {
            return;
        }
        let air = Vec3::from_array(crate::scene::jka_position(frame.wind_cm_s)) / self.scale;
        let (curl_seed, frequency, pan) = self.kind.curl();
        let curl_offset = Vec3::new(0.65, 0.125, 0.37) * (4096.0 * seeded_noise(curl_seed, self.seed, 0, 1));
        let fuel = frame.fuel.clamp(0.0, 1.0);
        let slots = self.kind.slots();
        let rates = slots.map(|(_, rate, _)| rate * self.scale * self.scale * fuel);
        let ticks = ((frame.dt * 60.0).ceil() as u32).clamp(1, MAX_TICKS);
        let step = frame.dt.min(MAX_TICKS as f32 / 60.0) / ticks as f32;
        for tick in 0..ticks {
            let from = previous.lerp(anchor, tick as f32 / ticks as f32);
            let to = previous.lerp(anchor, (tick + 1) as f32 / ticks as f32);
            let time = frame.time - step * (ticks - tick - 1) as f32;
            self.ticks = self.ticks.wrapping_add(1);
            let floats: [f32; 24] = [
                from.x, from.y, from.z, step,
                to.x, to.y, to.z, time,
                air.x, air.y, air.z, 0.0,
                curl_offset.x, curl_offset.y, curl_offset.z, frequency,
                pan.x, pan.y, pan.z, 0.0,
                rates[0], rates[1], slots[0].0 as f32, slots[1].0 as f32,
            ];
            let mut bytes = Vec::with_capacity(PARAMS_BYTES as usize);
            bytes.extend_from_slice(bytemuck::cast_slice(&floats));
            bytes.extend_from_slice(bytemuck::cast_slice(&[self.capacity, self.seed, self.brush_count, self.ticks]));
            let offset = PARAMS_STRIDE * u64::from(tick);
            queue.write_buffer(&self.params, offset, &bytes);
            let source = if self.ticked { self.current } else { 0 };
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Ninja fire particles"),
                timestamp_writes: None,
            });
            pass.set_bind_group(0, &self.sim_groups[source], &[offset as u32]);
            pass.set_bind_group(1, &self.args_group, &[]);
            pass.set_pipeline(&compute.begin);
            pass.dispatch_workgroups(1, 1, 1);
            // The update and spawn layouts have one group; the arguments group is inactive there.
            pass.set_pipeline(&compute.update);
            pass.dispatch_workgroups_indirect(&self.args, 0);
            pass.set_pipeline(&compute.spawn);
            pass.dispatch_workgroups_indirect(&self.args, 12);
            pass.set_pipeline(&compute.end);
            pass.dispatch_workgroups(1, 1, 1);
            drop(pass);
            self.current = 1 - source;
            self.ticked = true;
        }
    }

    fn to_local(&self, render_cm: Vec3) -> Vec3 {
        let d = (render_cm - self.frame_cm) / self.scale;
        Vec3::new(d.x, -d.z, d.y)
    }

    /// Upload the solid brushes reachable from the emitter's recent positions,
    /// in the local frame; only when that neighbourhood or the map changes.
    fn update_brushes(
        &mut self, device: &wgpu::Device, compute: &EmberCompute, queue: &wgpu::Queue,
        brushes: Option<&[WorldBrush]>,
    ) {
        let Some(brushes) = brushes else {
            self.brush_count = 0;
            self.gathered = None;
            return;
        };
        let key = brushes.as_ptr() as usize ^ brushes.len();
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for &(_, anchor) in &self.history {
            min = min.min(anchor - Vec3::new(REACH_XY, REACH_XY, REACH_DOWN));
            max = max.max(anchor + Vec3::new(REACH_XY, REACH_XY, REACH_UP));
        }
        if !min.is_finite() || self.gathered.is_some_and(|(gathered, low, high)| {
            gathered == key && low.cmple(min).all() && high.cmpge(max).all()
        }) {
            return;
        }
        // A margin so a moving source does not gather every frame.
        min -= Vec3::splat(REACH_XY);
        max += Vec3::splat(REACH_XY);
        let (f, s) = (self.frame_cm, self.scale);
        // Local (x, y, z) maps to render (x, z, -y).
        let render_min = f + Vec3::new(min.x, min.z, -max.y) * s;
        let render_max = f + Vec3::new(max.x, max.z, -min.y) * s;
        let (packed, count) = pack_brushes(
            brushes.iter().filter(|brush| brush.mins.cmple(render_max).all() && brush.maxs.cmpge(render_min).all()),
            |brush| (
                Vec3::new((brush.mins.x - f.x) / s, -(brush.maxs.z - f.z) / s, (brush.mins.y - f.y) / s),
                Vec3::new((brush.maxs.x - f.x) / s, -(brush.mins.z - f.z) / s, (brush.maxs.y - f.y) / s),
            ),
            |n, d| [n.x, -n.z, n.y, (d - n.dot(f)) / s],
        );
        if self.brushes.write(device, queue, "Ninja fire particle brushes", &packed) {
            self.sim_groups = sim_groups(
                device, compute, &self.curl, &self.params, &self.buffers, &self.counters, &self.brushes.buffer,
            );
        }
        self.brush_count = count;
        self.gathered = Some((key, min, max));
    }

    /// Group 1 of `ember_vertex`; the caller binds the flame uniform at group 0.
    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if self.ticked && self.awake {
            pass.set_bind_group(1, &self.draw_groups[self.current], &[]);
            pass.draw_indirect(&self.args, DRAW_ARGS);
        }
    }
}

fn sim_groups(
    device: &wgpu::Device, compute: &EmberCompute, curl: &wgpu::TextureView, params: &wgpu::Buffer,
    buffers: &[wgpu::Buffer; 2], counters: &wgpu::Buffer, brushes: &wgpu::Buffer,
) -> [wgpu::BindGroup; 2] {
    let group = |source: usize| device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Ninja fire particle simulation"),
        layout: &compute.sim_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: params, offset: 0, size: wgpu::BufferSize::new(PARAMS_BYTES),
                }),
            },
            wgpu::BindGroupEntry { binding: 1, resource: buffers[source].as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: buffers[1 - source].as_entire_binding() },
            wgpu::BindGroupEntry { binding: 3, resource: counters.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 4, resource: brushes.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(curl) },
            wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&compute.wrap) },
        ],
    });
    [group(0), group(1)]
}

// NiagaraEmitterInstanceShader.usf rand3.x, with a stable host emitter seed
// (the WGSL `em_random` is the same function).
fn seeded_noise(a: u32, b: u32, c: u32, counter: u32) -> f32 {
    let [mut x, mut y, mut z] = [a, b, counter | (c << 16)]
        .map(|seed| seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223));
    for _ in 0..2 {
        x = x.wrapping_add(y.wrapping_mul(z));
        y = y.wrapping_add(z.wrapping_mul(x));
        z = z.wrapping_add(x.wrapping_mul(y));
    }
    (x >> 8) as f32 / 16_777_216.0
}

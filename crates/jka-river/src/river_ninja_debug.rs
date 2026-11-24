use super::{ActorKind, ContactField, Domain, FrameSource, GpuTrack, WakeDebugSubject, checked_scope};
use jka_ninja::{RiverField, WakeTrack, WakeTrackSource};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};
use wgpu::{BindingResource, BindingType, BufferBindingType, BufferUsages, TextureViewDimension};

type GpuSummary = [[f32; 4]; 13];
const BYTES: u64 = size_of::<GpuSummary>() as u64;
const INTERVAL: Duration = Duration::from_secs(1);

pub(super) struct Probe {
    contacts: wgpu::ComputePipeline,
    solver: wgpu::ComputePipeline,
    selection: wgpu::Buffer,
    result: wgpu::Buffer,
    staging: wgpu::Buffer,
    pending: Option<Pending>,
    last_sample: Option<Instant>,
    last_poll: Instant,
    last_tick: Option<(Instant, Option<f64>)>,
    ticks: u32,
    sample: u64,
}

struct Pending {
    ready: Receiver<Result<(), wgpu::BufferAsyncError>>,
    river: u64,
    sample: u64,
    begin: usize,
    tracks: Vec<(WakeTrack, GpuTrack)>,
    solver_sampled: bool,
}

impl Probe {
    pub(super) fn new(device: &wgpu::Device) -> Result<Self, String> {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ninja river wake probe"),
            source: wgpu::ShaderSource::Wgsl(include_str!("river_ninja_debug.wgsl").into()),
        });
        let pipeline = |name, bindings: &[(u32, BindingType)]| {
            let entries: Vec<_> = bindings.iter().map(|&(binding, ty)| wgpu::BindGroupLayoutEntry {
                binding, ty, visibility: wgpu::ShaderStages::COMPUTE, count: None,
            }).collect();
            let group = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(name), entries: &entries,
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(name), bind_group_layouts: &[Some(&group)], immediate_size: 0,
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name), layout: Some(&layout), module: &shader,
                entry_point: Some(name), compilation_options: Default::default(), cache: None,
            })
        };
        let buffer_type = |ty| BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None };
        let uniform = buffer_type(BufferBindingType::Uniform);
        let read = buffer_type(BufferBindingType::Storage { read_only: true });
        let write = buffer_type(BufferBindingType::Storage { read_only: false });
        let texture = |view_dimension| BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension, multisampled: false,
        };
        let d2 = texture(TextureViewDimension::D2);
        let contacts = pipeline("probe_contacts", &[
            (0, uniform), (1, texture(TextureViewDimension::D2Array)), (2, read),
            (3, read), (4, uniform), (5, write),
        ]);
        let solver = pipeline("probe_solver", &[
            (3, read), (5, write), (6, uniform), (7, read), (8, read), (9, read),
            (10, d2), (11, d2), (12, d2), (13, d2),
        ]);
        let buffer = |label, size, usage| device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label), size, usage, mapped_at_creation: false,
        });
        let selection = buffer("Ninja wake probe selection", 16, BufferUsages::UNIFORM | BufferUsages::COPY_DST);
        let result = buffer("Ninja wake probe summary", BYTES, BufferUsages::STORAGE | BufferUsages::COPY_SRC | BufferUsages::COPY_DST);
        let staging = buffer("Ninja wake probe readback", BYTES, BufferUsages::COPY_DST | BufferUsages::MAP_READ);
        checked_scope(scope)?;
        Ok(Self {
            contacts, solver, selection, result, staging, pending: None, last_sample: None,
            last_poll: Instant::now(), last_tick: None, ticks: 0, sample: 0,
        })
    }

    pub(super) fn frame(
        &mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder,
        state: &wgpu::Texture, source: FrameSource<'_>, subject: WakeDebugSubject,
        river: u64, domain: Domain, field_step: Option<(f64, bool)>, material_bound: bool,
        tracks: &[WakeTrack], contacts: Option<&ContactField>, field: Option<&RiverField>,
    ) -> Result<(), String> {
        let now = Instant::now();
        let stepped = field_step.is_some_and(|(_, stepped)| stepped);
        if stepped {
            self.ticks += 1;
            self.last_tick = Some((now, source.game_seconds));
        }
        self.poll(device, now);
        if self.pending.is_some() || self.last_sample.is_some_and(|t| now.duration_since(t) < INTERVAL) {
            return Ok(());
        }
        // Prefer a consumed tick so flags, hash lists, and painter deltas
        // describe the same frame. Emit contact-only summaries if time stalls.
        if !stepped && self.last_tick.is_some_and(|(t, _)| now.duration_since(t) < Duration::from_millis(150)) {
            return Ok(());
        }
        self.last_sample = Some(now);
        self.sample += 1;
        let sample = self.sample;
        let ticks = std::mem::take(&mut self.ticks);
        let player = subject.client.and_then(|id| source.tracks.iter()
            .find(|track| track.current.id == id && track.current.kind == ActorKind::Player));
        let position = player.map(|track| track.current.origin_map).or(subject.position_map);
        let joints = player.and_then(|track| track.current.river_bones)
            .map_or(0, |bones| bones.into_iter().flatten().count());
        let skeletal = tracks.iter().filter(|t| t.slot != 0).count();
        let primitive = tracks.len() - skeletal;
        let local_hull = player.is_some() && tracks.iter().any(|t| Some(t.actor_id) == subject.client && t.slot == 0);
        eprintln!(
            "[RIVER NINJA WAKE] river={river} sample={sample} sources={} client={:?} local_actor={} map={position:?} joints={joints} local_hull={local_hull} expanded={}/{skeletal}/{primitive} domain_cm={:?} extent_cm={:?} game_s={:?} game_delta_s={:?} ticks={ticks} stepped={stepped} last_tick_game_s={:?} material_bound={material_bound}",
            source.tracks.len(), subject.client, player.is_some(), tracks.len(), domain.origin_cm,
            domain.extent_cm, source.game_seconds, field_step.map(|(delta, _)| delta),
            self.last_tick.and_then(|(_, time)| time),
        );

        let Some(contacts) = contacts.filter(|c| c.count > 0) else { return Ok(()); };
        let selected_actor = player.map(|t| t.current.id).or_else(|| source.tracks.iter()
            .find(|t| t.current.kind == ActorKind::Player).map(|t| t.current.id));
        let begin = tracks.iter().position(|t| Some(t.actor_id) == selected_actor).unwrap_or(0);
        let count = tracks[begin..].iter().take_while(|t| t.actor_id == tracks[begin].actor_id).count();
        let state_view = state.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(TextureViewDimension::D2Array), ..Default::default()
        });
        let binding = |binding, resource| wgpu::BindGroupEntry { binding, resource };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let contact_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Ninja wake probe contact inputs"), layout: &self.contacts.get_bind_group_layout(0),
            entries: &[
                binding(0, contacts.controls.as_entire_binding()),
                binding(1, BindingResource::TextureView(&state_view)),
                binding(2, contacts.tracks.as_entire_binding()),
                binding(3, contacts.flags.as_entire_binding()),
                binding(4, self.selection.as_entire_binding()),
                binding(5, self.result.as_entire_binding()),
            ],
        });
        let solver_group = field.filter(|_| stepped).and_then(|field| {
            let out = field.outputs();
            let probe = field.wake_debug();
            Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Ninja wake probe solver inputs"), layout: &self.solver.get_bind_group_layout(0),
                entries: &[
                    binding(3, contacts.flags.as_entire_binding()),
                    binding(5, self.result.as_entire_binding()),
                    binding(6, probe.hash_params.as_entire_binding()),
                    binding(7, probe.strokes.as_entire_binding()),
                    binding(8, probe.global.as_entire_binding()),
                    binding(9, probe.neighbors.as_entire_binding()),
                    binding(10, BindingResource::TextureView(probe.painter_before)),
                    binding(11, BindingResource::TextureView(probe.painter_after)),
                    binding(12, BindingResource::TextureView(out.velocity_density?)),
                    binding(13, BindingResource::TextureView(out.pressure?)),
                ],
            }))
        });
        checked_scope(scope)?;

        source.queue.write_buffer(&self.selection, 0, bytemuck::cast_slice(&[begin as u32, count as u32, 0, 0]));
        encoder.clear_buffer(&self.result, 0, None);
        dispatch(encoder, &self.contacts, &contact_group);
        if let Some(group) = &solver_group { dispatch(encoder, &self.solver, group); }
        encoder.copy_buffer_to_buffer(&self.result, 0, &self.staging, 0, BYTES);
        let (send, ready) = mpsc::channel();
        encoder.map_buffer_on_submit(&self.staging, wgpu::MapMode::Read, .., move |result| {
            let _ = send.send(result);
        });
        self.pending = Some(Pending {
            ready, river, sample, begin, solver_sampled: solver_group.is_some(),
            tracks: tracks[begin..begin + count].iter().copied()
                .zip(contacts.records[begin..begin + count].iter().copied()).collect(),
        });
        Ok(())
    }

    fn poll(&mut self, device: &wgpu::Device, now: Instant) {
        let Some(pending) = &self.pending else { return };
        if now.duration_since(self.last_poll) >= Duration::from_millis(100) {
            let _ = device.poll(wgpu::PollType::Poll);
            self.last_poll = now;
        }
        let ready = match pending.ready.try_recv() {
            Ok(ready) => ready,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending = None;
                return;
            }
        };
        let pending = self.pending.take().expect("readback in progress");
        if let Err(error) = ready {
            eprintln!("[RIVER NINJA WAKE] river={} sample={} map failed: {error}", pending.river, pending.sample);
            return;
        }
        let data = self.staging.slice(..).get_mapped_range()
            .map(|view| bytemuck::pod_read_unaligned::<GpuSummary>(&view));
        self.staging.unmap();
        match data {
            Ok(summary) => pending.report(summary),
            Err(error) => eprintln!("[RIVER NINJA WAKE] river={} sample={} readback failed: {error:?}", pending.river, pending.sample),
        }
    }
}

fn dispatch(encoder: &mut wgpu::CommandEncoder, pipeline: &wgpu::ComputePipeline, group: &wgpu::BindGroup) {
    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("Ninja river wake probe"), timestamp_writes: None,
    });
    pass.set_pipeline(pipeline);
    pass.set_bind_group(0, group, &[]);
    pass.dispatch_workgroups(1, 1, 1);
}

impl Pending {
    fn report(self, data: GpuSummary) {
        let Self { river, sample, begin, tracks, solver_sampled, .. } = self;
        let [total, subject, index, reason] = data[0];
        let [x, y, depth, height] = data[1];
        let Some((track, record)) = (index as usize).checked_sub(begin).and_then(|i| tracks.get(i)) else { return };
        let reason = ["outside_xy", "dry", "water_below_band", "water_above_band", "within_band"][reason as usize];
        let (source, length_cm) = match track.source {
            WakeTrackSource::Skeletal { parent_distance_cm } => ("joint", parent_distance_cm),
            WakeTrackSource::Primitive { bounds_radius_cm } => ("hull_center", bounds_radius_cm),
        };
        eprintln!(
            "[RIVER NINJA WAKE] river={river} sample={sample} contact_flags={total:.0} actor={} slot={} actor_flags={subject:.0}/{} source={source} source_length_cm={length_cm:.2} pos_cm={:?} previous_cm={:?} time_ms={} continuity={} column=({x:.0},{y:.0}) depth_cm={depth:.2} height_cm={height:.2} band_cm=[{:.2},{:.2}] column_result={reason}",
            track.actor_id, track.slot, tracks.len(), track.origin_cm, track.previous_cm,
            track.time_ms, track.continuity, record.mins[2], record.maxs[2],
        );
        if !solver_sampled { return; }
        let [uploaded, admitted, passed, stored] = data[2];
        let [nx, ny, occupied, full] = data[3];
        let [neighbors, listed, radius, overflow] = data[4];
        eprintln!(
            "[RIVER NINJA WAKE] river={river} sample={sample} hash1 uploaded={uploaded:.0} xy_admitted={admitted:.0} contact_passed={passed:.0} stored={stored:.0} overflow={:.0} hash2 grid={nx:.0}x{ny:.0} occupied={occupied:.0} full={full:.0} selected_cell_neighbors={neighbors:.0} selected_listed={listed:.0} selected_cell_overflow={overflow:.0} unit_radius={radius:.6} brush_unit_xyz_size={:?} brush_velocity_xyz_keep={:?}",
            data[10][3], data[5], data[6],
        );
        if data[5][0] <= 0.0 || data[5][0] >= 1.0 || data[5][1] <= 0.0 || data[5][1] >= 1.0 { return; }
        eprintln!(
            "[RIVER NINJA WAKE] river={river} sample={sample} painter_roi_5x5 max_abs_rgb_and_positive_b_before={:?} after={:?} max_abs_delta_rgb_and_nonfinite_texels={:?} sum_abs_rgb={:?} export_velocity_density={:?} export_pressure_divergence={:?}",
            data[7], data[8], data[9], &data[10][..3], data[11], &data[12][..2],
        );
    }
}

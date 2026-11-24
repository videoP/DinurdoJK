//! Host side of the Coast whitewater particles (Water_Dense_Sea "PARTICLES 100m
//! simdriven - whitewater", SurfaceAlignedParticles). The private provider owns
//! the simulation WGSL (spawn/update ported from UE's generated GPU HLSL) and the
//! sprite material; this module owns the particle storage, the per-tick dispatch
//! and the sprite pipeline drawn right after the Coast surface.
use blue_water_gpu::original_surface::OriginalSurfacePipeline;
use jka_river::river_ninja::checked_scope;

const PARTICLE_BYTES: u64 = 80;
const ARGS_BYTES: u64 = 40;
/// UE's Niagara GPU tick is the engine frame and the emitter graph is per tick
/// (AddVelocity adds a fixed amount per tick while drag is per second), so its
/// look depends on the frame rate. UE 5.6.1 PIE ran the reference level at ~49
/// fps (editor log, tools/coast/capture_whitewater_probe.py); above that rate
/// the host accumulates frame time and ticks at most this often.
const MIN_TICK_SECONDS: f32 = 1.0 / 50.0;
/// Byte offsets into the argument buffer (coast_whitewater_sim.wgsl `ww_args`).
const UPDATE_ARGS: u64 = 0;
const SPAWN_ARGS: u64 = 12;
pub(super) const DRAW_ARGS: u64 = 24;

/// Pipelines and constant resources shared by every Coast pool.
pub(super) struct Shared {
    sim_layout: wgpu::BindGroupLayout,
    args_layout: wgpu::BindGroupLayout,
    begin: wgpu::ComputePipeline,
    update: wgpu::ComputePipeline,
    spawn: wgpu::ComputePipeline,
    end: wgpu::ComputePipeline,
    draw_layout: wgpu::BindGroupLayout,
    pub pipeline: wgpu::RenderPipeline,
    curl: wgpu::TextureView,
    wrap: wgpu::Sampler,
    capacity: u32,
}

fn entry(binding: u32, visibility: wgpu::ShaderStages, ty: wgpu::BindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility, ty, count: None }
}
fn storage_ty(read_only: bool) -> wgpu::BindingType {
    wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None }
}
fn uniform_ty() -> wgpu::BindingType {
    wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }
}
fn texture_ty(dimension: wgpu::TextureViewDimension) -> wgpu::BindingType {
    wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: dimension, multisampled: false }
}

impl Shared {
    /// `surface` is the composed Coast surface module (it carries `ninja_ww_vertex`/`ninja_ww_fragment`).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, base: &OriginalSurfacePipeline, waves: &wgpu::BindGroupLayout,
        surface: &wgpu::ShaderModule, format: wgpu::TextureFormat, samples: u32) -> Result<Option<Self>, String> {
        let (Some(source), Some(curl_bytes)) = (jka_ninja::coast_whitewater_sim_wgsl(), jka_ninja::coast_whitewater_curl_field()) else {
            return Ok(None);
        };
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let c = wgpu::ShaderStages::COMPUTE;
        let sim_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("Coast whitewater simulation"),
            entries: &[entry(0, c, uniform_ty()), entry(1, c, storage_ty(true)), entry(2, c, storage_ty(false)),
                entry(3, c, storage_ty(false)),
                entry(5, c, texture_ty(wgpu::TextureViewDimension::D2)), entry(6, c, texture_ty(wgpu::TextureViewDimension::D2)),
                entry(7, c, texture_ty(wgpu::TextureViewDimension::D2)), entry(8, c, texture_ty(wgpu::TextureViewDimension::D3)),
                entry(9, c, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
                entry(10, c, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
                entry(11, c, texture_ty(wgpu::TextureViewDimension::D2))] });
        let args_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("Coast whitewater arguments"),
            entries: &[entry(0, c, storage_ty(false))] });
        let sim_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("Coast whitewater simulation"),
            bind_group_layouts: &[Some(&sim_layout)], immediate_size: 0 });
        let args_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("Coast whitewater bookkeeping"),
            bind_group_layouts: &[Some(&sim_layout), Some(&args_layout)], immediate_size: 0 });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("Coast whitewater simulation"),
            source: wgpu::ShaderSource::Wgsl(source.into()) });
        let make = |entry: &str, layout: &wgpu::PipelineLayout| device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry), layout: Some(layout), module: &module, entry_point: Some(entry), compilation_options: Default::default(), cache: None });
        let (begin, update, spawn, end) = (make("ww_begin", &args_pl), make("ww_update_main", &sim_pl),
            make("ww_spawn_main", &sim_pl), make("ww_end", &args_pl));
        let vf = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("Coast whitewater sprites"),
            entries: &[entry(16, vf, storage_ty(true)), entry(17, vf, uniform_ty())] });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("Coast whitewater sprites"),
            bind_group_layouts: &[Some(&base.camera_layout), Some(&base.surface_layout), Some(waves), Some(&base.environment_layout),
                Some(&draw_layout)], immediate_size: 0 });
        // MI_SpriteMaterial_Light2 overrides BLEND_Translucent: straight alpha over the scene.
        let blend = wgpu::BlendState {
            color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add },
            alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add },
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor { label: Some("Coast whitewater sprites"),
            layout: Some(&layout),
            vertex: wgpu::VertexState { module: surface, entry_point: Some("ninja_ww_vertex"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: surface, entry_point: Some("ninja_ww_fragment"), compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL })] }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual), stencil: Default::default(), bias: Default::default() }),
            multisample: wgpu::MultisampleState { count: samples, ..Default::default() }, multiview_mask: None, cache: None });
        let extent = wgpu::Extent3d { width: 32, height: 32, depth_or_array_layers: 32 };
        if curl_bytes.len() != 32 * 32 * 32 * 8 { return Err("whitewater curl field size".into()); }
        let curl = device.create_texture(&wgpu::TextureDescriptor { label: Some("TilingCurl32"), size: extent, mip_level_count: 1,
            sample_count: 1, dimension: wgpu::TextureDimension::D3, format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] });
        queue.write_texture(curl.as_image_copy(), curl_bytes,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(32 * 8), rows_per_image: Some(32) }, extent);
        let wrap = device.create_sampler(&wgpu::SamplerDescriptor { label: Some("Coast whitewater curl wrap"),
            address_mode_u: wgpu::AddressMode::Repeat, address_mode_v: wgpu::AddressMode::Repeat, address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let v = checked_scope(validation); let m = checked_scope(memory); v?; m?;
        Ok(Some(Self { sim_layout, args_layout, begin, update, spawn, end, draw_layout, pipeline, curl: curl.create_view(&Default::default()), wrap,
            capacity: jka_ninja::COAST_WHITEWATER_CAPACITY }))
    }
}

/// One pool's particle state (ping-pong storage, counters, indirect arguments).
pub(super) struct Particles {
    buffers: [wgpu::Buffer; 2],
    counters: wgpu::Buffer,
    pub args: wgpu::Buffer,
    params: wgpu::Buffer,
    /// Sprite-pass uniform (clamp plane); held by the draw groups.
    draw: wgpu::Buffer,
    draw_groups: [wgpu::BindGroup; 2],
    args_group: wgpu::BindGroup,
    sim_groups: Option<([wgpu::TextureView; 4], [wgpu::BindGroup; 2])>,
    /// Index of the buffer the last tick wrote (what the sprites draw).
    current: usize,
    ticked: bool,
    previous: Option<[f32; 3]>,
    frame: u32,
    /// Frame time not yet simulated (MIN_TICK_SECONDS).
    pending: f32,
}

/// Per-tick inputs, UE cm in the Coast frame.
pub(super) struct Tick<'a> {
    pub delta: f32,
    pub time: f32,
    pub trace: [f32; 3],
    pub trace_size_m: f32,
    pub owner: [f32; 3],
    pub clamp_cm: f32,
    /// r_ninjaCoastDebug (sprite debug views 9/10).
    pub debug: f32,
    /// r_ninjaCoastWhitewater: scale on UE's spawn rate (host density control; 0 off).
    pub spawn_scale: f32,
    /// r_ninjaCoastWhitewaterOpacity: host scale on the sprites' displayed alpha.
    pub opacity: f32,
    /// r_ninjaCoastShoreDepth (UE cm, 0 = everywhere): spawn only over a bed shallower than this.
    pub shore_cm: f32,
    pub inputs: [&'a wgpu::TextureView; 4],
}

impl Particles {
    pub fn new(device: &wgpu::Device, shared: &Shared, clamp_cm: f32, queue: &wgpu::Queue) -> Result<Self, String> {
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let storage = wgpu::BufferUsages::STORAGE;
        let size = u64::from(shared.capacity) * PARTICLE_BYTES;
        let make = |label, size, usage| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false });
        let buffers = [make("Coast whitewater particles A", size, storage), make("Coast whitewater particles B", size, storage)];
        let counters = make("Coast whitewater counters", 32, storage | wgpu::BufferUsages::COPY_DST);
        let args = make("Coast whitewater indirect", ARGS_BYTES, storage | wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST);
        let params = make("Coast whitewater params", 80, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
        let draw = make("Coast whitewater draw", 16, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
        queue.write_buffer(&counters, 0, &[0u8; 32]);
        queue.write_buffer(&args, 0, &[0u8; ARGS_BYTES as usize]);
        queue.write_buffer(&draw, 0, bytemuck::cast_slice(&[clamp_cm, 0.0, 1.0, 0.0]));
        let draw_group = |buffer: &wgpu::Buffer| device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("Coast whitewater sprites"),
            layout: &shared.draw_layout, entries: &[
                wgpu::BindGroupEntry { binding: 16, resource: buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 17, resource: draw.as_entire_binding() }] });
        let draw_groups = [draw_group(&buffers[0]), draw_group(&buffers[1])];
        let args_group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("Coast whitewater arguments"),
            layout: &shared.args_layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: args.as_entire_binding() }] });
        let v = checked_scope(validation); let m = checked_scope(memory); v?; m?;
        Ok(Self { buffers, counters, args, params, draw, draw_groups, args_group, sim_groups: None, current: 0, ticked: false, previous: None, frame: 0, pending: 0.0 })
    }

    /// Bind group of the buffer the last tick wrote; None before the first tick.
    pub fn draw_group(&self) -> Option<&wgpu::BindGroup> {
        self.ticked.then(|| &self.draw_groups[self.current])
    }

    pub fn encode(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, shared: &Shared,
        clamp_sampler: &wgpu::Sampler, tick: &Tick<'_>) {
        queue.write_buffer(&self.draw, 0, bytemuck::cast_slice(&[tick.clamp_cm, tick.debug, tick.opacity, 0.0]));
        if !(tick.delta > 0.0) { return; }
        self.pending = (self.pending + tick.delta).min(0.25);
        if self.pending < MIN_TICK_SECONDS { return; }
        let delta = std::mem::take(&mut self.pending);
        let inputs = tick.inputs.map(|v| v.clone());
        if self.sim_groups.as_ref().is_none_or(|(v, _)| *v != inputs) {
            let group = |src: usize| device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("Coast whitewater simulation"),
                layout: &shared.sim_layout, entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: self.buffers[src].as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: self.buffers[1 - src].as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: self.counters.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&inputs[0]) },
                    wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(&inputs[1]) },
                    wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(&inputs[2]) },
                    wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(&shared.curl) },
                    wgpu::BindGroupEntry { binding: 9, resource: wgpu::BindingResource::Sampler(clamp_sampler) },
                    wgpu::BindGroupEntry { binding: 10, resource: wgpu::BindingResource::Sampler(&shared.wrap) },
                    wgpu::BindGroupEntry { binding: 11, resource: wgpu::BindingResource::TextureView(&inputs[3]) }] });
            let groups = [group(0), group(1)];
            self.sim_groups = Some((inputs, groups));
        }
        let previous = self.previous.unwrap_or(tick.trace);
        self.previous = Some(tick.trace);
        self.frame = self.frame.wrapping_add(1);
        let p = tick.trace;
        let params: [[f32; 4]; 4] = [[p[0], p[1], p[2], delta], [tick.trace_size_m, tick.trace_size_m, 1.0, tick.time],
            [tick.owner[0], tick.owner[1], tick.owner[2], tick.clamp_cm],
            [previous[0], previous[1], previous[2], jka_ninja::COAST_WHITEWATER_SPAWN_RATE * tick.spawn_scale]];
        let mut bytes = Vec::with_capacity(80);
        bytes.extend_from_slice(bytemuck::cast_slice(&params));
        bytes.extend_from_slice(bytemuck::cast_slice(&[shared.capacity, self.frame, 0x2f6b_1d3au32, tick.shore_cm.max(0.0).to_bits()]));
        queue.write_buffer(&self.params, 0, &bytes);
        let src = if self.ticked { self.current } else { 0 };
        let group = &self.sim_groups.as_ref().expect("whitewater groups").1[src];
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Coast whitewater"), timestamp_writes: None });
        pass.set_bind_group(0, group, &[]);
        pass.set_bind_group(1, &self.args_group, &[]);
        pass.set_pipeline(&shared.begin); pass.dispatch_workgroups(1, 1, 1);
        // Update/spawn layouts have one group, so the bound arguments group is inactive there.
        pass.set_pipeline(&shared.update); pass.dispatch_workgroups_indirect(&self.args, UPDATE_ARGS);
        pass.set_pipeline(&shared.spawn); pass.dispatch_workgroups_indirect(&self.args, SPAWN_ARGS);
        pass.set_pipeline(&shared.end); pass.dispatch_workgroups(1, 1, 1);
        drop(pass);
        self.current = 1 - src;
        self.ticked = true;
    }

}

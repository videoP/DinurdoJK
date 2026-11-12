//! Native river whitewater (Grid2D_SW_River's `SW_River_Secondary_Emitter`)
//! on the GPU. One persistent 50000-particle buffer (4 MB), one compute
//! dispatch per render frame and one instanced sprite draw (river_spray.wgsl,
//! in the river surface module). No CPU readback. See
//! `jka_water::river::secondary` for the port record and its deviations.
use jka_water::river::secondary;
use wgpu::util::DeviceExt;

const CONTROLS_BYTES: u64 = 208;

/// The camera as the ViewRecycler reads it, in river axes (JKA/Unreal Z up)
/// and centimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SprayView {
    pub position_cm: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    pub forward: [f32; 3],
    /// ViewToClip[0][0] = 1 / tan(horizontal half FOV).
    pub m00: f32,
    /// ViewToClip[1][1] = 1 / tan(vertical half FOV).
    pub m11: f32,
    /// View width / height.
    pub aspect: f32,
}

impl SprayView {
    /// The same basis and projection the river view uniform is built from.
    pub fn from_camera(camera: &jka_render_core::camera::Camera, size: [u32; 2]) -> Self {
        let forward = camera.forward();
        let roll = glam::Quat::from_axis_angle(forward, camera.roll);
        let right = forward.cross(glam::Vec3::Y).normalize_or_zero();
        let up = roll * right.cross(forward).normalize_or_zero();
        let right = roll * right;
        let river = |v: glam::Vec3| jka_render_core::coordinates::jka_position(v.to_array());
        let [w, h] = size.map(|n| n.max(1));
        let aspect = w as f32 / h as f32;
        let tan_half_y = (camera.fov_y_for_viewport(w, h) * 0.5).tan();
        Self {
            position_cm: river(camera.position * jka_water::river::CENTIMETRES_PER_MAP_UNIT),
            right: river(right),
            up: river(up),
            forward: river(forward),
            m00: 1.0 / (aspect * tan_half_y),
            m11: 1.0 / tan_half_y,
            aspect,
        }
    }

    fn words(&self) -> [f32; 16] {
        let [p, r, u, f] = [self.position_cm, self.right, self.up, self.forward];
        [
            p[0], p[1], p[2], 0.0,
            r[0], r[1], r[2], self.m00,
            u[0], u[1], u[2], self.m11,
            f[0], f[1], f[2], self.aspect,
        ]
    }

    fn valid(&self) -> bool {
        self.words().iter().all(|v| v.is_finite()) && self.m00 > 0.0 && self.m11 > 0.0 && self.aspect > 0.0
    }
}

/// Grid placement the emitter samples (WorldToUnit, Emitter.dx, User.SimRes,
/// LocalToWorldTranslation), for the unrotated native actor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Actor origin: XY centre and minimum Z, centimetres.
    pub origin_cm: [f64; 3],
    pub world_extent: [f32; 2],
    pub sim_res: [f32; 2],
    pub dx: f32,
}

/// The emitter's Grid2DCollectionReaders: the hydraulic state arrays (height,
/// depth, vx, vy) at either parity and the material's pre-blur NormalGrid.
pub struct Inputs<'a> {
    pub state: &'a [wgpu::Texture; 2],
    pub normal_grid: &'a wgpu::Texture,
}

pub struct Emitter {
    particles: wgpu::Buffer,
    uniform: wgpu::Buffer,
    update: wgpu::ComputePipeline,
    sampler: wgpu::Sampler,
    groups: [Option<wgpu::BindGroup>; 2],
    spawned: bool,
    tick: u32,
    age: f32,
    previous: Option<SprayView>,
    depth_range: [f32; 2],
}

impl Emitter {
    pub fn new(device: &wgpu::Device) -> Self {
        let particles = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("river whitewater particles"),
            size: u64::from(secondary::PARTICLE_COUNT) * secondary::PARTICLE_BYTES,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("river whitewater controls"),
            size: CONTROLS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("river whitewater SW_River_Secondary_Emitter"),
            source: wgpu::ShaderSource::Wgsl(secondary::shader().into()),
        });
        let update = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("river whitewater spawn/update"),
            layout: None,
            module: &module,
            entry_point: Some("update"),
            compilation_options: Default::default(),
            cache: None,
        });
        Self {
            particles,
            uniform,
            update,
            sampler: grid_sampler(device),
            groups: [None, None],
            spawned: false,
            tick: 0,
            age: 0.0,
            previous: None,
            depth_range: secondary::RESTRICT_DEPTH,
        }
    }

    /// Bind (or rebind) the reader textures; particles are kept.
    pub fn bind(&mut self, device: &wgpu::Device, inputs: Inputs<'_>) {
        let normal = inputs.normal_grid.create_view(&Default::default());
        self.groups = std::array::from_fn(|parity| {
            let state = inputs.state[parity].create_view(&Default::default());
            Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("river whitewater update"),
                layout: &self.update.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&state) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&normal) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 4, resource: self.particles.as_entire_binding() },
                ],
            }))
        });
    }

    pub fn is_bound(&self) -> bool {
        self.groups[0].is_some()
    }

    /// After hydraulics freeze only one state parity survives.
    pub fn retain_state(&mut self, state_index: usize) {
        for (i, group) in self.groups.iter_mut().enumerate() {
            if i != state_index {
                *group = None;
            }
        }
    }

    /// Authored minimum depth (cm); see `foam_field::authored_depth_range`.
    pub fn set_minimum_depth(&mut self, minimum_cm: f32) {
        self.depth_range = jka_water::river::foam_field::authored_depth_range(minimum_cm);
    }

    /// One render frame: the 50000-particle burst on the first call (spawn and
    /// update in one dispatch, as Niagara does), then ParticleUpdate. The
    /// control copy keeps queued frames independent.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        state_index: usize,
        placement: Placement,
        view: SprayView,
        delta_seconds: f32,
    ) -> Result<(), &'static str> {
        let Some(Some(group)) = self.groups.get(state_index) else {
            return Err("river whitewater reader binding is not retained");
        };
        if !view.valid() || !(delta_seconds.is_finite() && delta_seconds > 0.0) {
            return Ok(());
        }
        if !(placement.dx > 0.0 && placement.sim_res.iter().all(|v| *v >= 1.0) && placement.world_extent.iter().all(|v| *v > 0.0)) {
            return Err("invalid river whitewater placement");
        }
        // Emitter.Age before this tick's delta, as Niagara advances it.
        self.age += delta_seconds;
        let previous = self.previous.unwrap_or(view);
        let o = placement.origin_cm;
        let mut words = [0.0f32; 52];
        words[0..4].copy_from_slice(&[o[0] as f32, o[1] as f32, 1.0 / placement.world_extent[0], 1.0 / placement.world_extent[1]]);
        words[4..8].copy_from_slice(&[placement.sim_res[0], placement.sim_res[1], placement.dx, placement.dx]);
        words[8..12].copy_from_slice(&[o[2] as f32, self.depth_range[0], self.depth_range[1], 0.0]);
        words[12..16].copy_from_slice(&[delta_seconds, self.age, 0.0, 0.0]);
        let counters = [self.tick, 0u32, u32::from(!self.spawned), secondary::PARTICLE_COUNT];
        for (i, c) in counters.iter().enumerate() {
            words[16 + i] = f32::from_bits(*c);
        }
        words[20..36].copy_from_slice(&view.words());
        words[36..52].copy_from_slice(&previous.words());
        let upload = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("river whitewater frame controls"),
            contents: bytemuck::cast_slice(&words),
            usage: wgpu::BufferUsages::COPY_SRC,
        });
        encoder.copy_buffer_to_buffer(&upload, 0, &self.uniform, 0, CONTROLS_BYTES);
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("river whitewater update"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.update);
        pass.set_bind_group(0, group, &[]);
        pass.dispatch_workgroups(secondary::PARTICLE_COUNT.div_ceil(64), 1, 1);
        drop(pass);
        self.spawned = true;
        self.tick = self.tick.wrapping_add(1);
        self.previous = Some(view);
        Ok(())
    }

    /// Particles have been spawned and updated at least once.
    pub fn ready(&self) -> bool {
        self.spawned
    }

    pub fn particles(&self) -> &wgpu::Buffer {
        &self.particles
    }

    pub fn logical_bytes(&self) -> u64 {
        u64::from(secondary::PARTICLE_COUNT) * secondary::PARTICLE_BYTES + CONTROLS_BYTES
    }
}

/// Niagara Grid2DCollectionReader sampling: SF_Bilinear, AM_Clamp.
fn grid_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("river whitewater reader SF_Bilinear AM_Clamp"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    })
}

/// Parity probe: Sample_Water + Emission_Criterias at world positions.
#[cfg(any(test, feature = "test-support"))]
pub struct ProbeResult {
    pub water_depth: f32,
    pub water_height: f32,
    pub curvature: f32,
    pub emit_amount: f32,
    pub spawn_velocity: [f32; 3],
    pub curl: f32,
    pub velocity: [f32; 2],
    pub depth_difference: f32,
    pub recycle: bool,
}

#[cfg(any(test, feature = "test-support"))]
impl Emitter {
    /// Blocking diagnostic: runs the `probe` entry point over `points`
    /// (world cm) with this emitter's last controls.
    pub fn probe(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        inputs: Inputs<'_>,
        state_index: usize,
        placement: Placement,
        delta_seconds: f32,
        points: &[[f32; 3]],
    ) -> Vec<ProbeResult> {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("river whitewater probe"),
            source: wgpu::ShaderSource::Wgsl(secondary::shader().into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("river whitewater probe"),
            layout: None,
            module: &module,
            entry_point: Some("probe"),
            compilation_options: Default::default(),
            cache: None,
        });
        let o = placement.origin_cm;
        let mut words = [0.0f32; 52];
        words[0..4].copy_from_slice(&[o[0] as f32, o[1] as f32, 1.0 / placement.world_extent[0], 1.0 / placement.world_extent[1]]);
        words[4..8].copy_from_slice(&[placement.sim_res[0], placement.sim_res[1], placement.dx, placement.dx]);
        words[8..12].copy_from_slice(&[o[2] as f32, self.depth_range[0], self.depth_range[1], 0.0]);
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("river whitewater probe controls"),
            contents: bytemuck::cast_slice(&words),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let mut slots = vec![[0.0f32; 4]; points.len() * 3];
        for (i, p) in points.iter().enumerate() {
            slots[i * 3] = [p[0], p[1], p[2], delta_seconds];
        }
        let storage = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("river whitewater probe slots"),
            contents: bytemuck::cast_slice(&slots),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("river whitewater probe readback"),
            size: storage.size(),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let state = inputs.state[state_index].create_view(&Default::default());
        let normal = inputs.normal_grid.create_view(&Default::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("river whitewater probe"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&state) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&normal) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 5, resource: storage.as_entire_binding() },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups((points.len() as u32).div_ceil(64), 1, 1);
        }
        encoder.copy_buffer_to_buffer(&storage, 0, &readback, 0, storage.size());
        queue.submit(Some(encoder.finish()));
        readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("probe readback");
        let data = readback.slice(..).get_mapped_range().expect("mapped readback");
        let values: &[[f32; 4]] = bytemuck::cast_slice(&data);
        values
            .chunks(3)
            .map(|c| ProbeResult {
                water_depth: c[0][0],
                water_height: c[0][1],
                curvature: c[0][2],
                emit_amount: c[0][3],
                spawn_velocity: [c[1][0], c[1][1], c[1][2]],
                curl: c[1][3],
                velocity: [c[2][0], c[2][1]],
                depth_difference: c[2][2],
                recycle: c[2][3] > 0.5,
            })
            .collect()
    }

    /// Blocking diagnostic readback of the particle buffer:
    /// (position, VisibilityTag) per particle.
    pub fn read_particles(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Vec<([f32; 3], i32)> {
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("river whitewater particle readback"),
            size: self.particles.size(),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_buffer_to_buffer(&self.particles, 0, &readback, 0, self.particles.size());
        queue.submit(Some(encoder.finish()));
        readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("particle readback");
        let data = readback.slice(..).get_mapped_range().expect("mapped readback");
        let words: &[u32] = bytemuck::cast_slice(&data);
        words
            .chunks(20)
            .map(|p| ([f32::from_bits(p[0]), f32::from_bits(p[1]), f32::from_bits(p[2])], p[16] as i32))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitewater_kernel_validates() {
        let module = wgpu::naga::front::wgsl::parse_str(&secondary::shader()).expect("whitewater WGSL parses");
        let info = wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .unwrap_or_else(|e| panic!("whitewater WGSL validates: {e:?}"));
        let _ = info;
        for entry in ["update", "probe"] {
            assert!(module.entry_points.iter().any(|e| e.name == entry), "missing {entry}");
        }
    }

    #[test]
    fn controls_and_particle_layout_match_the_shader() {
        assert_eq!(CONTROLS_BYTES, 13 * 16);
        assert_eq!(secondary::PARTICLE_BYTES, 5 * 16);
    }

    #[test]
    fn river_surface_module_has_the_sprite_entry_points() {
        let module = crate::river_surface::compose().expect("river surface shader composes");
        wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("river surface shader validates: {e:?}"));
        for entry in ["vs_spray", "fs_spray"] {
            assert!(module.entry_points.iter().any(|e| e.name == entry), "missing entry point {entry}");
        }
    }

    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        loop {
            if let std::task::Poll::Ready(value) = future.as_mut().poll(&mut context) {
                return value;
            }
            std::thread::yield_now();
        }
    }

    fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
        block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await.ok()?;
            if !adapter.features().contains(wgpu::Features::FLOAT32_FILTERABLE) {
                return None;
            }
            adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_features: wgpu::Features::FLOAT32_FILTERABLE,
                    required_limits: adapter.limits(),
                    ..Default::default()
                })
                .await
                .ok()
        })
    }

    const CELLS: u32 = 64;
    const DX: f32 = 25.0;

    /// Synthetic pourover (secondary.rs CPU test): 300 cm water over a bed at
    /// 100 cm, a 60 cm drop over three cells at the centre column, 150 cm/s
    /// along +X. Cell-centred values.
    fn height(i: u32) -> f32 {
        let t = ((i as f32 - 30.0) / 3.0).clamp(0.0, 1.0);
        300.0 - 60.0 * t
    }
    fn normal(i: u32) -> [f32; 3] {
        let slope = (height((i + 1).min(CELLS - 1)) - height(i.saturating_sub(1))) / (2.0 * DX);
        let len = (slope * slope + 1.0).sqrt();
        [-slope / len, 0.0, 1.0 / len]
    }

    struct Pourover {
        state: [wgpu::Texture; 2],
        normal_grid: wgpu::Texture,
        placement: Placement,
    }

    fn pourover(device: &wgpu::Device, queue: &wgpu::Queue) -> Pourover {
        let size = wgpu::Extent3d { width: CELLS, height: CELLS, depth_or_array_layers: 4 };
        let state = std::array::from_fn(|_| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("synthetic river state"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: crate::river_gpu::WATER_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        });
        let n = (CELLS * CELLS) as usize;
        let mut layers = vec![0.0f32; n * 4];
        for j in 0..CELLS {
            for i in 0..CELLS {
                let at = (j * CELLS + i) as usize;
                layers[at] = height(i);
                layers[n + at] = height(i) - 100.0;
                layers[2 * n + at] = 150.0;
                layers[3 * n + at] = 0.0;
            }
        }
        for texture in &state {
            queue.write_texture(
                texture.as_image_copy(),
                bytemuck::cast_slice(&layers),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(CELLS * 4), rows_per_image: Some(CELLS) },
                size,
            );
        }
        // Full-precision normals keep the curvature comparison exact.
        let normal_grid = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("synthetic NormalGrid"),
            size: wgpu::Extent3d { width: CELLS, height: CELLS, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let normals: Vec<[f32; 4]> = (0..CELLS * CELLS).map(|k| {
            let [x, y, z] = normal(k % CELLS);
            [x, y, z, 0.0]
        }).collect();
        queue.write_texture(
            normal_grid.as_image_copy(),
            bytemuck::cast_slice(&normals),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(CELLS * 16), rows_per_image: Some(CELLS) },
            wgpu::Extent3d { width: CELLS, height: CELLS, depth_or_array_layers: 1 },
        );
        let extent = CELLS as f32 * DX;
        Pourover {
            state,
            normal_grid,
            placement: Placement { origin_cm: [0.0, 0.0, 0.0], world_extent: [extent, extent], sim_res: [CELLS as f32; 2], dx: DX },
        }
    }

    /// Cell-centred bilinear clamp, as the reader sampler.
    struct CpuFields;
    impl jka_water::river::foam_field::Fields for CpuFields {
        fn depth(&self, unit: [f32; 2]) -> f32 {
            lerp_x(unit, |i| height(i) - 100.0)
        }
        fn velocity(&self, _unit: [f32; 2]) -> [f32; 2] {
            [150.0, 0.0]
        }
        fn normal(&self, unit: [f32; 2]) -> [f32; 3] {
            std::array::from_fn(|c| lerp_x(unit, |i| normal(i)[c]))
        }
    }
    fn lerp_x(unit: [f32; 2], value: impl Fn(u32) -> f32) -> f32 {
        let x = unit[0] * CELLS as f32 - 0.5;
        let i0 = x.floor();
        let f = x - i0;
        let at = |i: f32| value(i.clamp(0.0, (CELLS - 1) as f32) as u32);
        at(i0) * (1.0 - f) + at(i0 + 1.0) * f
    }

    #[test]
    fn gpu_emission_probe_matches_cpu_oracle() {
        let Some((device, queue)) = device() else {
            eprintln!("no FLOAT32_FILTERABLE GPU adapter; whitewater probe skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let synthetic = pourover(&device, &queue);
        let emitter = Emitter::new(&device);
        let extent = synthetic.placement.world_extent[0];
        // Cell centres of the middle row: every neighbour tap is a centre too.
        let points: Vec<[f32; 3]> = (2..CELLS - 2)
            .map(|i| [((i as f32 + 0.5) / CELLS as f32 - 0.5) * extent, ((32.5) / CELLS as f32 - 0.5) * extent, 0.0])
            .collect();
        let dt = 1.0 / 60.0;
        let inputs = Inputs { state: &synthetic.state, normal_grid: &synthetic.normal_grid };
        let gpu = emitter.probe(&device, &queue, inputs, 0, synthetic.placement, dt, &points);
        let setup = jka_water::river::foam_field::Setup {
            dimensions: [CELLS, CELLS],
            dx: DX,
            foam_dx: DX,
            sim_res: [CELLS as f32; 2],
            unit_to_world: [[0.0; 4]; 4],
            settings: Default::default(),
        };
        let frame = jka_water::river::foam_field::Frame::new(dt, 0.0);
        let mut emitting = 0;
        for (i, (point, g)) in points.iter().zip(&gpu).enumerate() {
            let unit = [point[0] / extent + 0.5, point[1] / extent + 0.5];
            let fields = CpuFields;
            let sample = secondary::WaterSample {
                emission: jka_water::river::foam_field::emission_inputs(&setup, frame, unit, &fields),
                water_height: lerp_x(unit, height),
                water_normal: jka_water::river::foam_field::Fields::normal(&fields, unit),
            };
            let cpu = secondary::emission(*point, sample, secondary::RESTRICT_DEPTH);
            let e = sample.emission;
            let close = |a: f32, b: f32, tol: f32| (a - b).abs() <= tol * (1.0 + b.abs());
            assert!(close(g.water_depth, e.water_depth, 1e-5) && close(g.water_height, sample.water_height, 1e-5), "cell {i}: depth/height");
            assert!(close(g.curvature, e.curvature, 1e-4) && close(g.curl, e.curl, 1e-4), "cell {i}: curvature {} vs {}", g.curvature, e.curvature);
            assert!(close(g.velocity[0], e.velocity[0], 1e-5), "cell {i}: velocity");
            assert!(close(g.emit_amount, cpu.emit_amount, 1e-3), "cell {i}: EmitAmount {} vs {}", g.emit_amount, cpu.emit_amount);
            for c in 0..3 {
                assert!(close(g.spawn_velocity[c], cpu.spawn_velocity[c], 1e-4), "cell {i}: spawn velocity");
            }
            if (cpu.emit_amount - secondary::MIN_EMIT_AMOUNT).abs() > 1e-3 {
                assert_eq!(g.recycle, cpu.recycle, "cell {i}: recycle");
            }
            emitting += usize::from(!cpu.recycle);
        }
        assert!(emitting > 0, "the synthetic lip never passes the emission criteria");
        let error = block_on(scope.pop());
        assert!(error.is_none(), "whitewater probe validation error: {error:?}");
    }

    #[test]
    fn gpu_emitter_spawns_spray_only_at_the_lip() {
        let Some((device, queue)) = device() else {
            eprintln!("no FLOAT32_FILTERABLE GPU adapter; whitewater emitter test skipped");
            return;
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let synthetic = pourover(&device, &queue);
        let mut emitter = Emitter::new(&device);
        emitter.bind(&device, Inputs { state: &synthetic.state, normal_grid: &synthetic.normal_grid });
        // Upstream of the lip, looking down the flow (river axes: right is -Y).
        let eye = [-900.0f32, 0.0, 600.0];
        let target = [0.0f32, 0.0, 280.0];
        let f = {
            let d = [target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]];
            let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            d.map(|v| v / l)
        };
        let right = {
            let r = [f[1], -f[0], 0.0];
            let l = (r[0] * r[0] + r[1] * r[1]).sqrt();
            r.map(|v| v / l)
        };
        let up = [right[1] * f[2] - right[2] * f[1], right[2] * f[0] - right[0] * f[2], right[0] * f[1] - right[1] * f[0]];
        let aspect = 16.0 / 9.0;
        let view = SprayView { position_cm: eye, right, up, forward: f, m00: 1.0, m11: aspect, aspect };
        for _ in 0..90 {
            let mut encoder = device.create_command_encoder(&Default::default());
            emitter.encode(&device, &mut encoder, 0, synthetic.placement, view, 1.0 / 60.0).expect("whitewater frame");
            queue.submit(Some(encoder.finish()));
        }
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("GPU frames");
        let particles = emitter.read_particles(&device, &queue);
        assert_eq!(particles.len(), secondary::PARTICLE_COUNT as usize);
        let visible: Vec<_> = particles.iter().filter(|(_, tag)| *tag == 0).map(|(p, _)| *p).collect();
        eprintln!("whitewater: {} of {} particles visible", visible.len(), particles.len());
        assert!(!visible.is_empty(), "no spray at the synthetic pourover");
        let extent = synthetic.placement.world_extent[0];
        // The emitting cell is column 29 (x = -25 cm); spray drifts with the
        // 150 cm/s flow for at most one 0.5 s lifetime.
        for p in &visible {
            assert!(p.iter().all(|v| v.is_finite()), "non-finite visible particle {p:?}");
            assert!(p[0] > -150.0 && p[0] < 200.0, "spray away from the lip: {p:?}");
            assert!(p[1].abs() < 0.5 * extent && p[2] > 150.0 && p[2] < 400.0, "spray off the water: {p:?}");
        }
        let error = block_on(scope.pop());
        assert!(error.is_none(), "whitewater validation error: {error:?}");
    }
}

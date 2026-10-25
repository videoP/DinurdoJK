//! Visual-only local waves. WebGPU Water's height/velocity stencil, with
//! Crest's world-space persistence, signed velocity inputs and edge damping.
//! Reference provenance and licenses: THIRD_PARTY.md.
use super::{OceanMasks, OceanSurface};
use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};
use wgpu::util::DeviceExt;

pub const SIZE: u32 = 256;
pub const LAYERS: usize = 4;
const CELL: f32 = 4.0;
const SPAN: f32 = SIZE as f32 * CELL;
const INPUTS: usize = 16;
const LIFETIME: f32 = 2.5;
const MAX_IMPACTS: usize = 32;
const MAX_SEGMENTS: usize = 128;
pub const SPLASH_INSTANCES_PER_EVENT: u32 = 30;
pub const SPLASH_EVENT_CAPACITY: u32 = MAX_IMPACTS as u32;
const SPLASH_LIFETIME: f32 = 1.5;

/// Captured once per presentation update, independently of rendered models.
#[derive(Clone, Copy, Debug)]
pub struct ActorSample {
    pub id: u16,
    pub origin: [f32; 3],
    pub head_height: f32,
    pub time_ms: i32,
    pub continuity: i32,
    /// Native/presentation velocity in render space, when available. Preserve
    /// the pre-contact sample even if movement has already stopped the fall.
    pub velocity: Option<[f32; 3]>,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
struct Domain {
    // Published origin XZ, inverse span, calm plane (all in world units).
    region: [f32; 4],
    bounds: [f32; 4],
    // enabled, debug, temporal fade, spare.
    controls: [f32; 4],
    identity: [f32; 4], // world clipmap slot + 1; zero for authored bounds
}

pub struct RenderResources {
    pub view: wgpu::TextureView,
    pub uniform: wgpu::Buffer,
    pub splash_uniform: wgpu::Buffer,
    _texture: wgpu::Texture,
}

impl RenderResources {
    pub fn new(device: &wgpu::Device) -> Self {
        Self::create(device, SIZE, LAYERS as u32)
    }

    pub fn inert(device: &wgpu::Device) -> Self {
        Self::create(device, 1, 1)
    }

    fn create(device: &wgpu::Device, size: u32, layers: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Ocean interaction published height/slopes"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ocean interaction published domains"),
            contents: bytemuck::cast_slice(&[Domain::default(); LAYERS]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let splash_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ocean interaction splash events"),
            contents: bytemuck::bytes_of(&SplashParams::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        Self {
            view,
            uniform,
            splash_uniform,
            _texture: texture,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
struct Input {
    position: [f32; 4], // XZ, radius, strength
    motion: [f32; 4],   // horizontal/vertical speed; w=1 for an instantaneous impact
}

#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
struct SplashEventGpu {
    position: [f32; 4], // world XYZ, normalized impact intensity
    motion: [f32; 4],   // incoming XYZ velocity, event time
    kind: [f32; 4],     // zero for entry, one for sustained wake spray
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SplashParams {
    controls: [f32; 4], // interaction time, style, event count, spare
    events: [SplashEventGpu; MAX_IMPACTS],
}

fn entry_splash_enabled(mode: u8) -> bool {
    matches!(mode, 1 | 3 | 4 | 5)
}

fn wake_spray_enabled(mode: u8) -> bool {
    matches!(mode, 2 | 3 | 5)
}

#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
struct Params {
    target: [f32; 4], // origin XZ, span, cell size
    source: [f32; 4], // previous origin XZ, reset, simulation dt
    bounds: [f32; 4],
    indices: [u32; 4], // layer, input count, mask first triangle, count
    mask_bounds: [f32; 4], // exact triangle bounds used by mask_tiles
    mask_tiles: [u32; 4], // 2-bit 8x8 exact footprint accelerator
    tuning: [f32; 4],  // velocity damping, height damping, edge fraction, spare
    inputs: [Input; INPUTS],
}

struct ActorHistory {
    sample: ActorSample,
    previous: ActorSample,
    velocity: Vec3,
    seen: f32,
    last_impact: Option<(OceanSurface, i32)>,
    last_spray: f32,
}

#[derive(Clone, Copy)]
struct MotionSegment {
    previous: ActorSample,
    current: ActorSample,
    previous_velocity: Vec3,
}

#[derive(Clone, Copy)]
struct Impact {
    body: OceanSurface,
    mask: Option<usize>,
    input: Input,
    born: f32,
}

fn native_velocity(sample: ActorSample, fallback: Vec3) -> Vec3 {
    sample
        .velocity
        .map(Vec3::from_array)
        .filter(|v| v.is_finite())
        .unwrap_or(fallback)
}

/// Feet crossing the nominal plane downward. Native velocity before collision
/// supplies momentum; position interpolation locates the actual crossing. The
/// solver receives a velocity jump, not a force multiplied by the tick length.
fn entry_impact(
    segment: MotionSegment,
    body: OceanSurface,
    masks: &OceanMasks,
    mask: Option<usize>,
) -> Option<Input> {
    let derived = movement(segment.previous, segment.current)?;
    let old_feet = segment.previous.origin[1] - 24.0;
    let new_feet = segment.current.origin[1] - 24.0;
    if old_feet <= body.plane_height || new_feet > body.plane_height || old_feet <= new_feet {
        return None;
    }
    let fraction = ((old_feet - body.plane_height) / (old_feet - new_feet)).clamp(0.0, 1.0);
    let origin = Vec3::from_array(segment.previous.origin)
        .lerp(Vec3::from_array(segment.current.origin), fraction);
    let mut crossing = segment.current;
    crossing.origin = [origin.x, body.plane_height + 24.0, origin.z];
    if !contact(crossing, body, masks, mask) {
        return None;
    }
    // Preserve one complete vector, rather than mixing component maxima from
    // different samples. Prefer native pre-contact data. A still-accelerating
    // current sample may improve it; a stopped post-contact sample cannot erase it.
    let mut incoming = native_velocity(
        segment.previous,
        if segment.previous_velocity.y < derived.y {
            segment.previous_velocity
        } else {
            derived
        },
    );
    let current = native_velocity(segment.current, derived);
    if current.y < incoming.y && current.length_squared() > incoming.length_squared() {
        incoming = current;
    }
    incoming = incoming.clamp_length_max(8192.0);
    let speed = incoming.length();
    if speed < 120.0 {
        return None;
    }
    // Calibrated visual coupling for fixed player mass. Normalize amplitude
    // by contact radius: integrated squared velocity scales with total speed^2,
    // even as faster entries generate a wider initial disturbance.
    let radius = (14.0 * (speed / 200.0).sqrt()).clamp(14.0, 64.0);
    let gain = 0.08 * (14.0 / radius);
    Some(Input {
        position: [origin.x, origin.z, radius, 1.0],
        motion: [
            incoming.x * gain,
            incoming.z * gain,
            incoming.y.min(0.0) * gain,
            1.0,
        ],
    })
}

/// Reject discontinuities before deriving velocity. No speed from render FPS.
pub(super) fn movement(previous: ActorSample, current: ActorSample) -> Option<Vec3> {
    let dt = (current.time_ms - previous.time_ms) as f32 * 0.001;
    let delta = Vec3::from_array(current.origin) - Vec3::from_array(previous.origin);
    if current.continuity != previous.continuity
        || !(0.0001..=0.25).contains(&dt)
        || !delta.is_finite()
        || delta.length_squared() > 192.0 * 192.0
    {
        return None;
    }
    Some(delta / dt)
}

fn contact(
    sample: ActorSample,
    surface: OceanSurface,
    masks: &OceanMasks,
    slot: Option<usize>,
) -> bool {
    let p = sample.origin;
    let feet = p[1] - 24.0;
    p.iter().all(|x| x.is_finite())
        && feet <= surface.plane_height + 1.0
        && p[1] + sample.head_height >= surface.plane_height - 1.0
        && p[0] >= surface.minimum[0]
        && p[0] <= surface.maximum[0]
        && p[2] >= surface.minimum[1]
        && p[2] <= surface.maximum[1]
        && slot.is_none_or(|i| masks.contains(i, [p[0], p[2]]))
}

fn swept_contact(
    previous: ActorSample,
    current: ActorSample,
    surface: OceanSurface,
    masks: &OceanMasks,
    slot: Option<usize>,
) -> bool {
    if contact(current, surface, masks, slot) || contact(previous, surface, masks, slot) {
        return true;
    }
    let feet = previous.origin[1].min(current.origin[1]) - 24.0;
    let head =
        (previous.origin[1] + previous.head_height).max(current.origin[1] + current.head_height);
    if feet > surface.plane_height || head < surface.plane_height {
        return false;
    }
    let mut at_surface = current;
    at_surface.origin[1] = surface.plane_height;
    contact(at_surface, surface, masks, slot)
}

struct Slot {
    body: Option<OceanSurface>,
    mask: Option<usize>,
    origin: [f32; 2],
    front: usize,
    valid: bool,
    last_input: f32,
    count: usize,
    inputs: [Input; INPUTS],
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            body: None,
            mask: None,
            origin: [0.0; 2],
            front: 0,
            valid: false,
            last_input: -LIFETIME,
            count: 0,
            inputs: [Input::default(); INPUTS],
        }
    }
}

struct Simulation {
    _state: [wgpu::Texture; 2],
    _coverage: wgpu::Buffer,
    triangles: wgpu::Buffer,
    params: [wgpu::Buffer; LAYERS],
    groups: Vec<[wgpu::BindGroup; 2]>,
    mask_pipeline: wgpu::ComputePipeline,
    step_pipeline: wgpu::ComputePipeline,
    publish_pipeline: wgpu::ComputePipeline,
}

impl Simulation {
    fn new(device: &wgpu::Device, render: &RenderResources) -> Self {
        let state = std::array::from_fn(|_| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Ocean interaction height/velocity state"),
                size: wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: LAYERS as u32,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rg32Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
                view_formats: &[],
            })
        });
        let views: [_; 2] = std::array::from_fn(|i| {
            state[i].create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        });
        let coverage = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ocean interaction cached dry-cell coverage"),
            size: u64::from(SIZE * SIZE) * LAYERS as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let triangles = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ocean interaction water outline"),
            size: super::OCEAN_MASK_VEC4S as u64 * 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let params = std::array::from_fn(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Ocean interaction frozen tick input"),
                size: std::mem::size_of::<Params>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        let texture = |binding, format| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format,
                view_dimension: wgpu::TextureViewDimension::D2Array,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ocean interaction compute bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                texture(1, wgpu::TextureFormat::Rg32Float),
                texture(2, wgpu::TextureFormat::Rgba16Float),
                super::storage_entry(3, false),
                super::storage_entry(4, true),
                super::uniform_entry(5),
            ],
        });
        let groups = (0..LAYERS)
            .map(|layer| {
                std::array::from_fn(|front| {
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("Ocean interaction ping-pong bindings"),
                        layout: &layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(&views[front]),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(&views[1 - front]),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::TextureView(&render.view),
                            },
                            super::bg_buffer(3, &coverage),
                            super::bg_buffer(4, &triangles),
                            super::bg_buffer(5, &params[layer]),
                        ],
                    })
                })
            })
            .collect();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ocean interactive wave port"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../ocean_interaction.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Ocean interaction compute layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: crate::pipeline_cache::get().as_ref(),
            })
        };
        Self {
            _state: state,
            _coverage: coverage,
            triangles,
            params,
            groups,
            mask_pipeline: pipeline("coverage"),
            step_pipeline: pipeline("step"),
            publish_pipeline: pipeline("publish"),
        }
    }
}

pub struct InteractionGpu {
    pub render: RenderResources,
    simulation: Option<Simulation>,
    actors: HashMap<u16, ActorHistory>,
    sources: Arc<Vec<ActorSample>>,
    segments: VecDeque<MotionSegment>,
    impacts: VecDeque<Impact>,
    splash_events: VecDeque<SplashEventGpu>,
    splash_serial: u32,
    slots: [Slot; LAYERS],
    domains: [Domain; LAYERS],
    masks: Arc<OceanMasks>,
    enabled: bool,
    strength: f32,
    splash_mode: u8,
    debug: bool,
    lifetime: f32,
    fade: f32,
    damping: f32,
    edge_fade: f32,
    clock: f32,
    next_tick: f32,
    started: f32,
    period: f32,
    phase: usize,
    jobs: Vec<(usize, usize)>,
    pending_origins: [[f32; 2]; LAYERS],
    last_sample_time: Option<i32>,
    reset_pending: bool,
    last_diagnostic: f32,
    pub updates: u64,
    pub input_count: usize,
    pub impact_events: u64,
    pub impacts_injected: u64,
}

impl InteractionGpu {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            render: RenderResources::new(device),
            simulation: None,
            actors: HashMap::new(),
            sources: Arc::new(Vec::new()),
            segments: VecDeque::with_capacity(MAX_SEGMENTS),
            impacts: VecDeque::with_capacity(MAX_IMPACTS),
            splash_events: VecDeque::with_capacity(MAX_IMPACTS),
            splash_serial: 1,
            slots: std::array::from_fn(|_| Slot::default()),
            domains: [Domain::default(); LAYERS],
            masks: Arc::new(OceanMasks::default()),
            enabled: false,
            strength: 1.0,
            splash_mode: 3,
            debug: false,
            lifetime: LIFETIME,
            fade: 1.0,
            damping: 2.0,
            edge_fade: 81.92,
            clock: 0.0,
            next_tick: 0.0,
            started: 0.0,
            period: 0.0,
            phase: 0,
            jobs: Vec::with_capacity(LAYERS * 3),
            pending_origins: [[0.0; 2]; LAYERS],
            last_sample_time: None,
            reset_pending: false,
            last_diagnostic: -1.0,
            updates: 0,
            input_count: 0,
            impact_events: 0,
            impacts_injected: 0,
        }
    }

    /// Cold settings path: no compilation or allocation in encode_frame.
    pub fn configure(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enabled: bool,
        strength: f32,
        debug: bool,
        splash_mode: u8,
    ) {
        self.strength = strength;
        self.debug = debug;
        self.splash_mode = splash_mode.min(5);
        if enabled != self.enabled {
            self.clear(queue);
            self.enabled = enabled;
            self.simulation = enabled.then(|| Simulation::new(device, &self.render));
        }
        self.upload_splash_events(queue);
    }

    pub fn clear(&mut self, queue: &wgpu::Queue) {
        self.actors.clear();
        self.segments.clear();
        self.impacts.clear();
        self.splash_events.clear();
        self.jobs.clear();
        self.phase = 0;
        self.slots = std::array::from_fn(|_| Slot::default());
        self.domains = [Domain::default(); LAYERS];
        self.last_sample_time = None;
        self.reset_pending = false;
        self.next_tick = self.clock;
        queue.write_buffer(&self.render.uniform, 0, bytemuck::cast_slice(&self.domains));
        self.upload_splash_events(queue);
    }

    fn upload_splash_events(&mut self, queue: &wgpu::Queue) {
        let clock = self.clock;
        self.splash_events
            .retain(|event| clock - event.motion[3] <= SPLASH_LIFETIME);
        let mut params = SplashParams::zeroed();
        params.controls = [
            self.clock,
            self.splash_mode as f32,
            self.splash_events.len() as f32,
            0.0,
        ];
        for (destination, event) in params.events.iter_mut().zip(self.splash_events.iter()) {
            *destination = *event;
        }
        queue.write_buffer(
            &self.render.splash_uniform,
            0,
            bytemuck::bytes_of(&params),
        );
    }

    fn push_splash_event(&mut self, mut event: SplashEventGpu) {
        if self.splash_events.len() == MAX_IMPACTS {
            self.splash_events.pop_front();
        }
        event.kind[1] = self.splash_serial as f32;
        self.splash_serial = self.splash_serial.wrapping_add(1);
        self.splash_events.push_back(event);
    }

    pub fn set_falloff(&mut self, settings: super::OceanSettings) {
        let settings = settings.sanitize();
        self.lifetime = settings.interaction_lifetime;
        self.fade = settings.interaction_fade;
        self.damping = settings.interaction_damping;
        self.edge_fade = settings.interaction_edge_fade;
    }

    pub fn set_sources(&mut self, sources: &Arc<Vec<ActorSample>>) {
        if !self.enabled || Arc::ptr_eq(&self.sources, sources) {
            return;
        }
        if let Some(time) = sources.first().map(|s| s.time_ms) {
            if self.last_sample_time.is_some_and(|old| time < old) {
                self.reset_pending = true;
                self.actors.clear();
            }
            self.last_sample_time = Some(time);
            for &sample in sources.iter().take(64) {
                let segments = &mut self.segments;
                self.actors
                    .entry(sample.id)
                    .and_modify(|history| {
                        if sample.time_ms != history.sample.time_ms
                            || sample.continuity != history.sample.continuity
                        {
                            if sample.origin[1] < history.sample.origin[1]
                                && movement(history.sample, sample).is_some()
                            {
                                if segments.len() == MAX_SEGMENTS {
                                    segments.pop_front();
                                }
                                segments.push_back(MotionSegment {
                                    previous: history.sample,
                                    current: sample,
                                    previous_velocity: history.velocity,
                                });
                            }
                            history.previous = history.sample;
                            history.velocity =
                                movement(history.sample, sample).unwrap_or(Vec3::ZERO);
                            history.sample = sample;
                        }
                        history.seen = self.clock;
                    })
                    .or_insert(ActorHistory {
                        sample,
                        previous: sample,
                        velocity: Vec3::ZERO,
                        seen: self.clock,
                        last_impact: None,
                        last_spray: -10.0,
                    });
            }
            self.actors.retain(|_, a| time - a.sample.time_ms <= 1000);
        }
        self.sources = Arc::clone(sources);
    }

    fn collect(&mut self, camera: Vec3, bodies: &[(OceanSurface, Option<usize>)]) {
        self.input_count = 0;
        for slot in &mut self.slots {
            slot.count = 0;
        }
        // Retained impacts have priority over sustained movement. Consume each
        // exactly once when freezing a solver tick, even across repeated renders.
        let pending = self.impacts.len();
        for _ in 0..pending {
            let Some(mut impact) = self.impacts.pop_front() else {
                break;
            };
            if self.clock - impact.born > 2.0
                || !bodies
                    .iter()
                    .any(|(b, m)| *b == impact.body && *m == impact.mask)
            {
                continue;
            }
            let Some(layer) = self.layer_for(impact.body, impact.mask) else {
                self.impacts.push_back(impact);
                continue;
            };
            let slot = &mut self.slots[layer];
            if slot.count == INPUTS {
                self.impacts.push_back(impact);
                continue;
            }
            impact.input.position[3] = self.strength;
            slot.inputs[slot.count] = impact.input;
            slot.count += 1;
            slot.last_input = self.clock;
            self.input_count += 1;
            self.impacts_injected += 1;
        }
        for source_index in 0..self.sources.len().min(64) {
            let sample = self.sources[source_index];
            if Vec3::from_array(sample.origin).distance_squared(camera) > (SPAN * 0.65).powi(2) {
                continue;
            }
            let Some((previous, velocity, seen, last_spray)) = self
                .actors
                .get(&sample.id)
                .map(|history| (history.previous, history.velocity, history.seen, history.last_spray))
            else {
                continue;
            };
            if self.clock - seen > 0.1 {
                continue;
            }
            if velocity.length_squared() < 8.0 * 8.0 {
                continue;
            }
            let Some(&(body, mask)) = bodies
                .iter()
                .find(|&&(body, mask)| swept_contact(previous, sample, body, &self.masks, mask))
            else {
                continue;
            };
            let v = velocity.clamp_length_max(1200.0);
            // Downward entry energy is handled by the discrete impulse. Avoid
            // repeating it as sustained heave after the crossing.
            let heave = v.y.max(0.0);
            if Vec3::new(v.x, heave, v.z).length_squared() < 64.0 {
                continue;
            }
            let Some(layer) = self.layer_for(body, mask) else {
                continue;
            };
            let slot = &mut self.slots[layer];
            if slot.count == INPUTS {
                continue;
            }
            slot.inputs[slot.count] = Input {
                position: [sample.origin[0], sample.origin[2], 14.0, self.strength],
                motion: [v.x, v.z, heave, 0.0],
            };
            slot.count += 1;
            slot.last_input = self.clock;
            self.input_count += 1;
            let horizontal_speed = Vec3::new(v.x, 0.0, v.z).length();
            if wake_spray_enabled(self.splash_mode)
                && horizontal_speed >= 40.0
                && self.clock - last_spray >= 0.14
            {
                let interaction_gain = (self.strength / 30.0).max(0.0).sqrt();
                let spray_strength = (horizontal_speed / 450.0 * interaction_gain).clamp(0.0, 2.0);
                if spray_strength > 0.01 {
                    self.push_splash_event(SplashEventGpu {
                        position: [
                            sample.origin[0],
                            body.plane_height,
                            sample.origin[2],
                            spray_strength,
                        ],
                        motion: [v.x, v.y, v.z, self.clock],
                        kind: [1.0, 0.0, 0.0, 0.0],
                    });
                    if let Some(history) = self.actors.get_mut(&sample.id) {
                        history.last_spray = self.clock;
                    }
                }
            }
        }
    }

    fn layer_for(&mut self, body: OceanSurface, mask: Option<usize>) -> Option<usize> {
        let layer = self
            .slots
            .iter()
            .position(|s| s.body == Some(body))
            .or_else(|| {
                self.slots
                    .iter()
                    .position(|s| s.body.is_none() || self.clock - s.last_input > self.lifetime)
            })?;
        if self.slots[layer].body != Some(body) {
            self.slots[layer] = Slot {
                body: Some(body),
                mask,
                ..Default::default()
            };
            self.domains[layer].controls[0] = 0.0;
        }
        Some(layer)
    }

    fn capture_impacts(&mut self, camera: Vec3, bodies: &[(OceanSurface, Option<usize>)]) {
        while let Some(segment) = self.segments.pop_front() {
            for &(body, mask) in bodies {
                let Some(input) = entry_impact(segment, body, &self.masks, mask) else {
                    continue;
                };
                if Vec3::new(input.position[0], body.plane_height, input.position[1])
                    .distance_squared(camera)
                    > (SPAN * 0.65).powi(2)
                {
                    continue;
                }
                let Some(history) = self.actors.get_mut(&segment.current.id) else {
                    continue;
                };
                if history
                    .last_impact
                    .is_some_and(|(old, time)| old == body && segment.current.time_ms - time < 150)
                {
                    continue;
                }
                history.last_impact = Some((body, segment.current.time_ms));
                if self.impacts.len() == MAX_IMPACTS {
                    self.impacts.pop_front();
                }
                if entry_splash_enabled(self.splash_mode) {
                    // The impact radius encodes incoming speed; recover that
                    // speed so FX energy follows the same contact evidence.
                    let impact_speed =
                        (200.0 * (input.position[2] / 14.0).powi(2)).clamp(120.0, 8192.0);
                    let interaction_gain = (self.strength / 30.0).max(0.0).sqrt();
                    let splash_strength = (impact_speed / 800.0 * interaction_gain).clamp(0.0, 2.0);
                    if splash_strength > 0.01 {
                        self.push_splash_event(SplashEventGpu {
                            position: [
                                input.position[0],
                                body.plane_height,
                                input.position[1],
                                splash_strength,
                            ],
                            motion: [
                                input.motion[0],
                                -input.motion[2],
                                input.motion[1],
                                self.clock,
                            ],
                            kind: [0.0; 4],
                        });
                    }
                }
                self.impacts.push_back(Impact {
                    body,
                    mask,
                    input,
                    born: self.clock,
                });
                self.impact_events += 1;
            }
        }
    }

    /// Use the existing ocean phase scheduler with bounded compute work. A cycle
    /// freezes its inputs until publish; render sees only complete height/slopes.
    pub fn encode_frame(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        delta: f32,
        hz: f32,
        camera: Vec3,
        bodies: &[(OceanSurface, Option<usize>)],
        masks: &Arc<OceanMasks>,
    ) {
        if !self.enabled {
            return;
        }
        self.clock += delta.clamp(0.0, 0.1);
        // Keep sub-millisecond scheduling precision during long high-FPS runs.
        // GPU parameters contain only relative time, so rebasing is invisible.
        if self.clock >= 60.0 {
            self.clock -= 60.0;
            self.next_tick -= 60.0;
            self.started -= 60.0;
            self.last_diagnostic -= 60.0;
            for slot in &mut self.slots {
                slot.last_input -= 60.0;
            }
            for actor in self.actors.values_mut() {
                actor.seen -= 60.0;
                actor.last_spray -= 60.0;
            }
            for impact in &mut self.impacts {
                impact.born -= 60.0;
            }
            for event in &mut self.splash_events {
                event.motion[3] -= 60.0;
            }
        }
        if self.reset_pending {
            self.clear(queue);
        }
        if !Arc::ptr_eq(&self.masks, masks) {
            self.clear(queue);
            self.masks = Arc::clone(masks);
            if !masks.triangles.is_empty() {
                queue.write_buffer(
                    &self.simulation.as_ref().unwrap().triangles,
                    0,
                    bytemuck::cast_slice(&masks.triangles),
                );
            }
        }
        self.capture_impacts(camera, bodies);
        self.upload_splash_events(queue);
        if self.jobs.is_empty() && self.clock >= self.next_tick {
            self.collect(camera, bodies);
            self.upload_splash_events(queue);
            if self.debug && self.clock - self.last_diagnostic >= 1.0 {
                eprintln!(
                    "ocean interaction: inputs={} active={} published={} hz={:.1} impacts={} injected={}",
                    self.input_count,
                    self.slots.iter().filter(|s| s.body.is_some()).count(),
                    self.updates,
                    hz, self.impact_events, self.impacts_injected
                );
                self.last_diagnostic = self.clock;
            }
            let tick_dt = if hz > 0.0 {
                1.0 / hz
            } else {
                (self.clock - self.started).clamp(0.0001, 0.1)
            };
            self.started = self.clock;
            self.period = if hz > 0.0 { 1.0 / hz } else { 0.0 };
            // Keep the requested cadence at high FPS without rounding every
            // interval up to a whole rendered frame. Drop excessive backlog.
            self.next_tick = (self.next_tick + self.period).max(self.clock - self.period);
            let simulation = self.simulation.as_ref().unwrap();
            for layer in 0..LAYERS {
                let slot = &mut self.slots[layer];
                if self.clock - slot.last_input > self.lifetime
                    || !bodies.iter().any(|(b, _)| Some(*b) == slot.body)
                {
                    slot.body = None;
                    slot.valid = false;
                    self.domains[layer].controls[0] = 0.0;
                    continue;
                }
                let age = self.clock - slot.last_input;
                let t = if self.fade > 0.0 {
                    ((age - (self.lifetime - self.fade)) / self.fade).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                self.domains[layer].controls[2] = 1.0 - t * t * (3.0 - 2.0 * t);
                let body = slot.body.unwrap();
                let desired = [
                    (camera.x / CELL).floor() * CELL - SPAN * 0.5,
                    (camera.z / CELL).floor() * CELL - SPAN * 0.5,
                ];
                // Recenter only every 64 world units; preserve exact cell overlap.
                let shifted =
                    !slot.valid || (0..2).any(|i| (desired[i] - slot.origin[i]).abs() >= 64.0);
                let origin = if shifted { desired } else { slot.origin };
                self.pending_origins[layer] = origin;
                let range = slot.mask.map_or([0.0; 4], |i| masks.ranges[i]);
                let params = Params {
                    target: [origin[0], origin[1], SPAN, CELL],
                    source: [
                        slot.origin[0],
                        slot.origin[1],
                        f32::from(!slot.valid),
                        tick_dt.min(1.0),
                    ],
                    bounds: [
                        body.minimum[0],
                        body.minimum[1],
                        body.maximum[0],
                        body.maximum[1],
                    ],
                    indices: [
                        layer as u32,
                        slot.count as u32,
                        range[0] as u32,
                        range[1] as u32,
                    ],
                    mask_bounds: slot.mask.map_or([0.0; 4], |i| masks.bounds[i]),
                    mask_tiles: slot.mask.map_or([0; 4], |i| masks.tiles[i]),
                    inputs: slot.inputs,
                    tuning: [
                        self.damping,
                        self.damping * 0.175,
                        self.edge_fade / SPAN,
                        0.0,
                    ],
                };
                queue.write_buffer(&simulation.params[layer], 0, bytemuck::bytes_of(&params));
                if shifted {
                    self.jobs.push((layer, 0));
                }
                self.jobs.push((layer, 1));
                self.jobs.push((layer, 2));
            }
            self.phase = 0;
            self.domains[0].controls[3] =
                self.domains.iter().filter(|d| d.controls[0] > 0.5).count() as f32;
            queue.write_buffer(&self.render.uniform, 0, bytemuck::cast_slice(&self.domains));
        }
        if self.jobs.is_empty() {
            return;
        }
        let due = if hz > 0.0 && delta > self.period * 0.35 {
            self.jobs.len()
        } else {
            super::ocean_phases_due(self.clock, self.started, self.period, self.jobs.len())
        };
        let budget =
            due.saturating_sub(self.phase)
                .min(if hz > 0.0 && delta > self.period * 0.35 {
                    3
                } else {
                    1
                });
        let simulation = self.simulation.as_ref().unwrap();
        for _ in 0..budget {
            let (layer, job) = self.jobs[self.phase];
            let slot = &mut self.slots[layer];
            let pipeline = match job {
                0 => &simulation.mask_pipeline,
                1 => &simulation.step_pipeline,
                _ => &simulation.publish_pipeline,
            };
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("Ocean interaction scheduled phase"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &simulation.groups[layer][slot.front], &[]);
                pass.dispatch_workgroups(SIZE / 8, SIZE / 8, 1);
            }
            // Step writes the other state. Publish reads that completed state.
            if job == 1 {
                slot.front = 1 - slot.front;
            }
            if job == 2 {
                let body = slot.body.unwrap();
                let origin = self.pending_origins[layer];
                slot.origin = origin;
                slot.valid = true;
                self.domains[layer] = Domain {
                    region: [origin[0], origin[1], 1.0 / SPAN, body.plane_height],
                    bounds: [
                        body.minimum[0],
                        body.minimum[1],
                        body.maximum[0],
                        body.maximum[1],
                    ],
                    controls: [
                        1.0,
                        f32::from(self.debug),
                        self.domains[layer].controls[2],
                        0.0,
                    ],
                    identity: [slot.mask.map_or(0.0, |i| (i + 1) as f32), 0.0, 0.0, 0.0],
                };
                self.domains[0].controls[3] =
                    self.domains.iter().filter(|d| d.controls[0] > 0.5).count() as f32;
                queue.write_buffer(&self.render.uniform, 0, bytemuck::cast_slice(&self.domains));
                self.updates += 1;
            }
            self.phase += 1;
        }
        if self.phase == self.jobs.len() {
            self.jobs.clear();
            self.phase = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn actor(time_ms: i32, x: f32, y: f32) -> ActorSample {
        ActorSample {
            id: 1,
            origin: [x, y, 0.0],
            head_height: 26.0,
            time_ms,
            continuity: 0,
            velocity: None,
        }
    }
    #[test]
    fn idle_and_discontinuities_do_not_inject() {
        assert_eq!(
            movement(actor(0, 0.0, 0.0), actor(20, 0.0, 0.0)),
            Some(Vec3::ZERO)
        );
        assert!(movement(actor(0, 0.0, 0.0), actor(20, 300.0, 0.0)).is_none());
        assert!(movement(actor(20, 0.0, 0.0), actor(0, 2.0, 0.0)).is_none());
        let mut reset = actor(20, 2.0, 0.0);
        reset.continuity = 8;
        assert!(movement(actor(0, 0.0, 0.0), reset).is_none());
    }
    #[test]
    fn contact_requires_surface_and_body_intersection() {
        let body = OceanSurface {
            plane_height: 0.0,
            minimum: [-100.0; 2],
            maximum: [100.0; 2],
        };
        let masks = OceanMasks::default();
        assert!(contact(actor(0, 0.0, 0.0), body, &masks, None));
        assert!(!contact(actor(0, 0.0, 100.0), body, &masks, None));
        assert!(!contact(actor(0, 0.0, -100.0), body, &masks, None));
        assert!(!contact(actor(0, 101.0, 0.0), body, &masks, None));
    }
    #[test]
    fn speed_uses_presentation_time() {
        assert!(
            (movement(actor(0, 0.0, 0.0), actor(20, 5.0, 0.0)).unwrap().x - 250.0).abs() < 0.01
        );
    }

    #[test]
    fn water_entry_uses_incoming_velocity_before_movement_stops() {
        let body = OceanSurface {
            plane_height: 0.0,
            minimum: [-100.0; 2],
            maximum: [100.0; 2],
        };
        let mut old = actor(0, 0.0, 25.0);
        old.velocity = Some([0.0, -1800.0, 0.0]);
        let mut current = actor(20, 0.0, 24.0);
        current.velocity = Some([0.0; 3]);
        let input = entry_impact(
            MotionSegment {
                previous: old,
                current,
                previous_velocity: Vec3::ZERO,
            },
            body,
            &OceanMasks::default(),
            None,
        )
        .unwrap();
        assert!((input.motion[2] + 48.0).abs() < 0.01);
        assert!(input.position[2] > 40.0);
        assert_eq!(input.motion[3], 1.0);
        old.velocity = Some([0.0, -200.0, 0.0]);
        let slow = entry_impact(
            MotionSegment {
                previous: old,
                current,
                previous_velocity: Vec3::ZERO,
            },
            body,
            &OceanMasks::default(),
            None,
        )
        .unwrap();
        assert!(input.motion[2].abs() > slow.motion[2].abs() * 2.9);
        assert!(input.position[2] > slow.position[2]);
        let fast_energy = (input.motion[2] * input.position[2]).powi(2);
        let slow_energy = (slow.motion[2] * slow.position[2]).powi(2);
        assert!((fast_energy / slow_energy - 81.0).abs() < 0.01);
    }

    #[test]
    fn diagonal_entry_preserves_total_velocity_and_direction() {
        let body = OceanSurface {
            plane_height: 0.0,
            minimum: [-100.0; 2],
            maximum: [100.0; 2],
        };
        let mut old = actor(0, 0.0, 25.0);
        let mut current = actor(20, 0.0, 24.0);
        current.velocity = Some([0.0; 3]);
        old.velocity = Some([0.0, -1800.0, 0.0]);
        let vertical = entry_impact(
            MotionSegment {
                previous: old,
                current,
                previous_velocity: Vec3::ZERO,
            },
            body,
            &OceanMasks::default(),
            None,
        )
        .unwrap();
        old.velocity = Some([1080.0, -1440.0, 0.0]);
        let diagonal = entry_impact(
            MotionSegment {
                previous: old,
                current,
                previous_velocity: Vec3::ZERO,
            },
            body,
            &OceanMasks::default(),
            None,
        )
        .unwrap();
        assert!((diagonal.position[2] - vertical.position[2]).abs() < 0.01);
        assert!(diagonal.motion[0] > 0.0);
        assert!(diagonal.motion[2].abs() < vertical.motion[2].abs());
        let energy = |input: Input| {
            Vec3::new(input.motion[0], input.motion[1], input.motion[2]).length_squared()
                * input.position[2].powi(2)
        };
        assert!((energy(vertical) / energy(diagonal) - 1.0).abs() < 0.0001);
    }

    #[test]
    fn impacts_reject_teleports_dry_ground_and_stationary_contacts() {
        let body = OceanSurface {
            plane_height: 0.0,
            minimum: [-100.0; 2],
            maximum: [100.0; 2],
        };
        let mut old = actor(0, 0.0, 25.0);
        old.velocity = Some([0.0, -1800.0, 0.0]);
        let mut current = actor(20, 0.0, 24.0);
        current.continuity = 8;
        assert!(entry_impact(
            MotionSegment {
                previous: old,
                current,
                previous_velocity: Vec3::ZERO
            },
            body,
            &OceanMasks::default(),
            None
        )
        .is_none());
        current.continuity = 0;
        current.origin[0] = 150.0;
        old.origin[0] = 150.0;
        assert!(entry_impact(
            MotionSegment {
                previous: old,
                current,
                previous_velocity: Vec3::ZERO
            },
            body,
            &OceanMasks::default(),
            None
        )
        .is_none());
        old = actor(0, 0.0, 24.0);
        current = actor(20, 0.0, 24.0);
        assert!(entry_impact(
            MotionSegment {
                previous: old,
                current,
                previous_velocity: Vec3::ZERO
            },
            body,
            &OceanMasks::default(),
            None
        )
        .is_none());
    }

    #[test]
    fn dry_holes_reject_contact() {
        let body = OceanSurface {
            plane_height: 0.0,
            minimum: [-100.0; 2],
            maximum: [100.0; 2],
        };
        let mut masks = OceanMasks::default();
        masks.push_surface(0, &[[[-100.0, -100.0], [0.0, -100.0], [-100.0, 100.0]]]);
        assert!(!contact(actor(0, 50.0, 0.0), body, &masks, Some(0)));
        assert!(contact(actor(0, -80.0, 0.0), body, &masks, Some(0)));
    }

    #[test]
    fn fast_vertical_crossing_still_contacts_the_surface() {
        let body = OceanSurface {
            plane_height: 0.0,
            minimum: [-100.0; 2],
            maximum: [100.0; 2],
        };
        let masks = OceanMasks::default();
        assert!(swept_contact(
            actor(0, 0.0, 50.0),
            actor(100, 0.0, -50.0),
            body,
            &masks,
            None
        ));
        assert!(!swept_contact(
            actor(0, 0.0, 150.0),
            actor(100, 0.0, 50.0),
            body,
            &masks,
            None
        ));
        assert!(!swept_contact(
            actor(0, 200.0, 50.0),
            actor(100, 200.0, -50.0),
            body,
            &masks,
            None
        ));
    }

    #[test]
    fn signed_contact_distinguishes_stacked_planes() {
        let low = OceanSurface {
            plane_height: 0.0,
            minimum: [-100.0; 2],
            maximum: [100.0; 2],
        };
        let high = OceanSurface {
            plane_height: 100.0,
            ..low
        };
        assert!(contact(
            actor(0, 0.0, 0.0),
            low,
            &OceanMasks::default(),
            None
        ));
        assert!(!contact(
            actor(0, 0.0, 0.0),
            high,
            &OceanMasks::default(),
            None
        ));
    }

    #[test]
    #[ignore = "requires a GPU; synchronous readback is confined to this test"]
    fn gpu_impulse_scroll_and_layer_isolation() {
        futures_lite::future::block_on(async {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
            let adapter = instance.request_adapter(&Default::default()).await.unwrap();
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_limits: adapter.limits(),
                    ..Default::default()
                })
                .await
                .unwrap();
            println!("interaction GPU: {:?}", adapter.get_info());
            let render = RenderResources::new(&device);
            let sim = Simulation::new(&device, &render);
            let mut params = Params {
                tuning: [2.0, 0.35, 0.08, 0.0],
                target: [-512.0, -512.0, SPAN, CELL],
                source: [-512.0, -512.0, 1.0, 1.0 / 48.0],
                bounds: [-256.0, -256.0, 256.0, 256.0],
                indices: [0, 1, 0, 0],
                ..Default::default()
            };
            params.inputs[0] = Input {
                position: [0.0, 0.0, 14.0, 1024.0],
                motion: [250.0, 0.0, -100.0, 0.0],
            };
            queue.write_buffer(&sim.params[0], 0, bytemuck::bytes_of(&params));
            let dispatch = |encoder: &mut wgpu::CommandEncoder,
                            pipeline: &wgpu::ComputePipeline,
                            front| {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &sim.groups[0][front], &[]);
                pass.dispatch_workgroups(SIZE / 8, SIZE / 8, 1);
            };
            let read = |render: &RenderResources| {
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: u64::from(SIZE * SIZE) * LAYERS as u64 * 8,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &render._texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(SIZE * 8),
                            rows_per_image: Some(SIZE),
                        },
                    },
                    wgpu::Extent3d {
                        width: SIZE,
                        height: SIZE,
                        depth_or_array_layers: LAYERS as u32,
                    },
                );
                queue.submit(Some(encoder.finish()));
                let (tx, rx) = std::sync::mpsc::channel();
                buffer
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |result| {
                        tx.send(result).unwrap();
                    });
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    })
                    .unwrap();
                rx.recv().unwrap().unwrap();
                let result = buffer.slice(..).get_mapped_range().expect("GPU readback range").to_vec();
                buffer.unmap();
                result
            };
            let mut encoder = device.create_command_encoder(&Default::default());
            dispatch(&mut encoder, &sim.mask_pipeline, 0);
            dispatch(&mut encoder, &sim.step_pipeline, 0);
            dispatch(&mut encoder, &sim.publish_pipeline, 1);
            queue.submit(Some(encoder.finish()));
            let first = read(&render);
            let stride = (SIZE * SIZE * 8) as usize;
            assert!(
                first[..stride]
                    .chunks_exact(8)
                    .any(|v| u16::from_le_bytes([v[0], v[1]]) & 0x7fff > 0x4800),
                "strong input must exceed the old eight-world-unit height ceiling"
            );
            assert!(
                first
                    .chunks_exact(2)
                    .all(|v| u16::from_le_bytes([v[0], v[1]]) & 0x7c00 != 0x7c00),
                "published half floats must remain finite"
            );
            assert!(
                first[..stride].iter().any(|&b| b != 0),
                "input must change height/slopes"
            );
            assert!(
                first[stride..].iter().all(|&b| b == 0),
                "unused water bodies remain flat"
            );
            assert!(
                first[..SIZE as usize * 8].iter().all(|&b| b == 0),
                "dry cells remain flat"
            );
            params.target[0] += 64.0;
            params.source[2] = 0.0;
            params.source[3] = 0.0;
            params.indices[1] = 0;
            queue.write_buffer(&sim.params[0], 0, bytemuck::bytes_of(&params));
            let mut encoder = device.create_command_encoder(&Default::default());
            dispatch(&mut encoder, &sim.mask_pipeline, 1);
            dispatch(&mut encoder, &sim.step_pipeline, 1);
            dispatch(&mut encoder, &sim.publish_pipeline, 0);
            queue.submit(Some(encoder.finish()));
            let second = read(&render);
            // 64 units = 16 cells. Compare a central patch at the same WORLD
            // coordinates, including its published gradients, bit for bit.
            for y in 110..146 {
                for x in 110..146 {
                    let old = (y * 256 + x) * 8;
                    let new = (y * 256 + x - 16) * 8;
                    assert_eq!(
                        &first[old..old + 8],
                        &second[new..new + 8],
                        "world-space scroll at {x},{y}"
                    );
                }
            }
            assert!(second[stride..].iter().all(|&b| b == 0));
            // An overlapping footprint on a separate body layer receives its
            // own impulse without changing the first body's published state.
            params.indices = [1, 1, 0, 0];
            params.source[2] = 1.0;
            params.source[3] = 1.0 / 48.0;
            params.inputs[0].motion[0] = -250.0;
            queue.write_buffer(&sim.params[1], 0, bytemuck::bytes_of(&params));
            let mut encoder = device.create_command_encoder(&Default::default());
            for (pipeline, front) in [
                (&sim.mask_pipeline, 0),
                (&sim.step_pipeline, 0),
                (&sim.publish_pipeline, 1),
            ] {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &sim.groups[1][front], &[]);
                pass.dispatch_workgroups(SIZE / 8, SIZE / 8, 1);
            }
            queue.submit(Some(encoder.finish()));
            let third = read(&render);
            assert_eq!(&third[..stride], &second[..stride]);
            assert!(third[stride..stride * 2].iter().any(|&b| b != 0));

            // Tangential entry momentum must change the shape, rather than
            // simply increasing an isotropic ring by total speed.
            params.indices = [2, 1, 0, 0];
            params.inputs[0] = Input {
                position: [0.0, 0.0, 40.0, 1.0],
                motion: [80.0, 0.0, -40.0, 1.0],
            };
            queue.write_buffer(&sim.params[2], 0, bytemuck::bytes_of(&params));
            let mut encoder = device.create_command_encoder(&Default::default());
            for (pipeline, front) in [
                (&sim.mask_pipeline, 0),
                (&sim.step_pipeline, 0),
                (&sim.publish_pipeline, 1),
            ] {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &sim.groups[2][front], &[]);
                pass.dispatch_workgroups(SIZE / 8, SIZE / 8, 1);
            }
            queue.submit(Some(encoder.finish()));
            let directional = read(&render);
            let at = |x: usize| {
                let offset = stride * 2 + (128 * 256 + x) * 8;
                u16::from_le_bytes([directional[offset], directional[offset + 1]])
            };
            assert!(
                at(115) & 0x7fff > 0 && at(115) & 0x8000 == 0,
                "forward side rises"
            );
            assert!(at(108) & 0x8000 != 0, "trailing side falls");

            // Capture a brief entry before a solver tick, followed immediately
            // by a stationary presentation sample. The event must survive and
            // be injected exactly once, including repeated renders of that pose.
            let mut field = InteractionGpu::new(&device);
            field.configure(&device, &queue, true, 30.0, false, 0);
            let body = OceanSurface {
                plane_height: 0.0,
                minimum: [-1000.0; 2],
                maximum: [1000.0; 2],
            };
            let masks = Arc::new(OceanMasks::default());
            let tick = |field: &mut InteractionGpu, delta| {
                let mut encoder = device.create_command_encoder(&Default::default());
                field.encode_frame(
                    &queue,
                    &mut encoder,
                    delta,
                    48.0,
                    Vec3::ZERO,
                    &[(body, None)],
                    &masks,
                );
                queue.submit(Some(encoder.finish()));
            };
            tick(&mut field, 0.001);
            let mut old = actor(0, 0.0, 25.0);
            old.velocity = Some([0.0, -1800.0, 0.0]);
            field.set_sources(&Arc::new(vec![old]));
            tick(&mut field, 0.001);
            let mut current = actor(20, 0.0, 24.0);
            current.velocity = Some([0.0; 3]);
            field.set_sources(&Arc::new(vec![current]));
            tick(&mut field, 0.001);
            assert_eq!(field.impact_events, 1);
            assert_eq!(
                field.impacts_injected, 0,
                "entry retained until the inherited tick"
            );
            current.time_ms = 40;
            field.set_sources(&Arc::new(vec![current]));
            for _ in 0..80 {
                tick(&mut field, 0.005);
            }
            assert_eq!(field.impact_events, 1);
            assert_eq!(field.impacts_injected, 1, "one entry produces one impulse");
            assert!(field.impacts.is_empty());
            assert!(
                read(&field.render)[..stride].iter().any(|&b| b != 0),
                "retained impact reaches the GPU field"
            );

            // Run the complete CPU/GPU scheduler, not a duplicate arithmetic
            // model: inherited 24/48 Hz must remain independent of 200/1000 FPS.
            for hz in [24.0, 48.0] {
                let mut counts = Vec::new();
                for frames in [200, 1000] {
                    let mut field = InteractionGpu::new(&device);
                    field.configure(&device, &queue, true, 1.0, false, 0);
                    let body = OceanSurface {
                        plane_height: 0.0,
                        minimum: [-1000.0; 2],
                        maximum: [1000.0; 2],
                    };
                    let masks = Arc::new(OceanMasks::default());
                    for frame in 0..frames {
                        let time = frame as f32 / frames as f32;
                        let source = Arc::new(vec![actor(
                            (time * 1000.0).round() as i32,
                            time * 100.0,
                            0.0,
                        )]);
                        field.set_sources(&source);
                        let mut encoder = device.create_command_encoder(&Default::default());
                        field.encode_frame(
                            &queue,
                            &mut encoder,
                            1.0 / frames as f32,
                            hz,
                            Vec3::ZERO,
                            &[(body, None)],
                            &masks,
                        );
                        queue.submit(Some(encoder.finish()));
                    }
                    counts.push(field.updates);
                    assert!(
                        (field.updates as f32 - hz).abs() <= 3.0,
                        "hz={hz}, frames={frames}, updates={}",
                        field.updates
                    );
                    // A source held at rest causes no new input and ages out.
                    for frame in 0..160 {
                        let source = Arc::new(vec![actor(1000 + frame * 20, 100.0, 0.0)]);
                        field.set_sources(&source);
                        let mut encoder = device.create_command_encoder(&Default::default());
                        field.encode_frame(
                            &queue,
                            &mut encoder,
                            0.02,
                            hz,
                            Vec3::ZERO,
                            &[(body, None)],
                            &masks,
                        );
                        queue.submit(Some(encoder.finish()));
                    }
                    assert_eq!(field.input_count, 0);
                    assert!(field.domains.iter().all(|d| d.controls[0] == 0.0));
                }
                assert!(
                    counts[0].abs_diff(counts[1]) <= 2,
                    "cadence at {hz} Hz: {counts:?}"
                );
            }
        });
    }
}

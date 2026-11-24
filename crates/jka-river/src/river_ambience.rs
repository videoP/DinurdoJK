//! Bounded, listener-centred GPU acoustic sampling. No render visibility test,
//! full-grid download, synchronous GPU wait, or modification of river physics.
use crate::{river_runtime::LiveRiver, river_surface::RiverSurface};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const SIDE: u32 = 16;
const CELL: f32 = 192.0; // JKA units, globally aligned
const MAX_RIVERS: usize = 8;
const ROW: u32 = SIDE * 16; // RGBA32Float, also 256-byte copy aligned
const PLANE: u64 = ROW as u64 * SIDE as u64 * MAX_RIVERS as u64;
const INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, Debug)]
pub struct Region {
    pub river: u64,
    pub bin: [i32; 2],
    pub origin: [f32; 3],
    pub coverage: f32,
    pub speed: f32,
    pub turbulence: f32,
    pub curl: f32,
    pub drop: f32,
}
#[derive(Clone, Copy)]
struct Request {
    eye: [f32; 3],
    epoch: u64,
    at: Instant,
}
#[derive(Clone)]
pub struct Snapshot {
    pub epoch: u64,
    pub at: Instant,
    pub eye: [f32; 3],
    pub regions: Arc<[Region]>,
}
static EPOCH: AtomicU64 = AtomicU64::new(1);
static EXCHANGE: Mutex<(Option<Request>, Option<Snapshot>)> = Mutex::new((None, None));

pub fn invalidate() {
    EPOCH.fetch_add(1, Ordering::AcqRel);
}
pub fn epoch() -> u64 {
    EPOCH.load(Ordering::Acquire)
}
/// Session thread, no waiting. Old map/teleport samples cannot be consumed.
pub fn request(eye: [f32; 3]) -> Option<Snapshot> {
    if !crate::river_runtime::audio_rivers_present() || !eye.iter().all(|v| v.is_finite()) {
        return None;
    }
    let now = Instant::now();
    let epoch = epoch();
    let mut exchange = EXCHANGE.try_lock().ok()?;
    exchange.0 = Some(Request {
        eye,
        epoch,
        at: now,
    });
    exchange
        .1
        .as_ref()
        .filter(|s| {
            s.epoch == epoch
                && s.at.elapsed() < Duration::from_millis(650)
                && s.eye
                    .iter()
                    .zip(eye)
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f32>()
                    < 384.0 * 384.0
        })
        .cloned()
}
fn listener() -> Option<Request> {
    EXCHANGE
        .try_lock()
        .ok()?
        .0
        .filter(|r| r.epoch == epoch() && r.at.elapsed() < Duration::from_millis(500))
}
struct Sample {
    request: Request,
    start: [i32; 2],
    rivers: Vec<u64>,
}
enum State {
    Idle,
    Copied(Sample),
    Mapping(Sample, mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>),
}

pub(super) struct Probe {
    pipeline: wgpu::RenderPipeline,
    params: wgpu::Buffer,
    group: wgpu::BindGroup,
    textures: [wgpu::Texture; 2],
    views: [wgpu::TextureView; 2],
    staging: wgpu::Buffer,
    state: State,
    last: Instant,
    last_poll: Instant,
}
impl Probe {
    fn new(device: &wgpu::Device, surface: &RiverSurface) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("river ambience area"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                count: None,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
            }],
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("river ambience area"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("river ambience area"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: params.as_entire_binding(),
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("river ambience"),
            bind_group_layouts: &[Some(&surface.body_layout), Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("river ambience"),
            source: wgpu::ShaderSource::Wgsl(include_str!("river_ambience.wgsl").into()),
        });
        let target = || {
            Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba32Float,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("river ambience"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[target(), target()],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let textures = std::array::from_fn(|_| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("river ambience metrics"),
                size: wgpu::Extent3d {
                    width: SIDE,
                    height: SIDE * MAX_RIVERS as u32,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        });
        let views = textures
            .each_ref()
            .map(|t| t.create_view(&Default::default()));
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("river ambience async readback"),
            size: PLANE * 2,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            params,
            group,
            textures,
            views,
            staging,
            state: State::Idle,
            last: Instant::now() - INTERVAL,
            last_poll: Instant::now(),
        }
    }
    pub(super) fn update(
        slot: &mut Option<Self>,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        surface: &RiverSurface,
        rivers: &[LiveRiver],
    ) {
        if let Some(probe) = slot {
            probe.poll(device);
        }
        let Some(request) = listener() else {
            return;
        };
        if rivers.is_empty() {
            return;
        }
        let probe = slot.get_or_insert_with(|| Self::new(device, surface));
        probe.encode(queue, encoder, rivers, request);
    }
    fn encode(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        rivers: &[LiveRiver],
        request: Request,
    ) {
        if !matches!(self.state, State::Idle) || self.last.elapsed() < INTERVAL {
            return;
        }
        let cm = jka_water::river::CENTIMETRES_PER_MAP_UNIT;
        let start = [0, 1].map(|i| (request.eye[i] / CELL).floor() as i32 - SIDE as i32 / 2);
        let lower = start.map(|x| x as f32 * CELL * cm);
        let upper = lower.map(|x| x + SIDE as f32 * CELL * cm);
        let mut nearby: Vec<_> = rivers
            .iter()
            .filter(|r| r.audio_enabled() && r.body_group().is_some())
            .filter_map(|r| {
                let size = r.mesh.geometry.size();
                let min = [0, 1].map(|i| r.origin_cm[i] as f32 - size[i] * 0.5);
                let max = [0, 1].map(|i| r.origin_cm[i] as f32 + size[i] * 0.5);
                if (0..2).any(|i| max[i] <= lower[i] || min[i] >= upper[i]) {
                    return None;
                }
                let d = (0..2)
                    .map(|i| {
                        let e = request.eye[i] * cm;
                        (e - e.clamp(min[i], max[i])).powi(2)
                    })
                    .sum::<f32>();
                Some((r, d))
            })
            .collect();
        nearby.sort_by(|(a, da), (b, db)| da.total_cmp(db).then(a.id.cmp(&b.id)));
        nearby.truncate(MAX_RIVERS);
        self.last = Instant::now();
        if nearby.is_empty() {
            publish(Snapshot {
                epoch: request.epoch,
                at: request.at,
                eye: request.eye,
                regions: Arc::from([]),
            });
            return;
        }
        queue.write_buffer(
            &self.params,
            0,
            bytemuck::cast_slice(&[lower[0], lower[1], CELL * cm, 0.0]),
        );
        {
            let attachments = self.views.each_ref().map(|view| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("river ambience metrics"),
                color_attachments: &attachments,
                depth_stencil_attachment: None,
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(1, &self.group, &[]);
            for (i, (river, _)) in nearby.iter().enumerate() {
                if let Some(body) = river.body_group() {
                    pass.set_bind_group(0, body, &[]);
                    pass.set_viewport(
                        0.0,
                        i as f32 * SIDE as f32,
                        SIDE as f32,
                        SIDE as f32,
                        0.0,
                        1.0,
                    );
                    pass.draw(0..3, 0..1);
                }
            }
        }
        let height = SIDE * nearby.len() as u32;
        for (i, texture) in self.textures.iter().enumerate() {
            encoder.copy_texture_to_buffer(
                texture.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &self.staging,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: PLANE * i as u64,
                        bytes_per_row: Some(ROW),
                        rows_per_image: Some(height),
                    },
                },
                wgpu::Extent3d {
                    width: SIDE,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        }
        self.state = State::Copied(Sample {
            request,
            start,
            rivers: nearby.iter().map(|(r, _)| r.id).collect(),
        });
    }
    fn poll(&mut self, device: &wgpu::Device) {
        if matches!(self.state, State::Copied(_)) {
            let State::Copied(sample) = std::mem::replace(&mut self.state, State::Idle) else {
                unreachable!()
            };
            let (tx, rx) = mpsc::channel();
            self.staging
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = tx.send(result);
                });
            self.state = State::Mapping(sample, rx);
        }
        let State::Mapping(_, rx) = &self.state else {
            return;
        };
        if self.last_poll.elapsed() >= Duration::from_millis(50) {
            let _ = device.poll(wgpu::PollType::Poll);
            self.last_poll = Instant::now();
        }
        let result = match rx.try_recv() {
            Ok(r) => r.is_ok(),
            Err(mpsc::TryRecvError::Empty) => return,
            Err(_) => false,
        };
        let State::Mapping(sample, _) = std::mem::replace(&mut self.state, State::Idle) else {
            unreachable!()
        };
        if result {
            if let Ok(mapped) = self.staging.slice(..).get_mapped_range() {
                let cm = jka_water::river::CENTIMETRES_PER_MAP_UNIT;
                let mut regions = Vec::new();
                let read = |offset: usize| {
                    f32::from_le_bytes(mapped[offset..offset + 4].try_into().unwrap())
                };
                for (i, &river) in sample.rivers.iter().enumerate() {
                    for y in 0..SIDE {
                        for x in 0..SIDE {
                            let index =
                                (i * SIDE as usize * SIDE as usize + (y * SIDE + x) as usize) * 16;
                            let a: [f32; 4] = std::array::from_fn(|k| read(index + k * 4));
                            let b: [f32; 4] =
                                std::array::from_fn(|k| read(PLANE as usize + index + k * 4));
                            if a[3] <= 0.0 || !a.iter().chain(b.iter()).all(|v| v.is_finite()) {
                                continue;
                            }
                            let bin = [sample.start[0] + x as i32, sample.start[1] + y as i32];
                            regions.push(Region {
                                river,
                                bin,
                                origin: [
                                    (bin[0] as f32 + b[2]) * CELL,
                                    (bin[1] as f32 + b[3]) * CELL,
                                    a[0] / cm,
                                ],
                                coverage: a[3],
                                speed: a[1],
                                turbulence: a[2],
                                curl: b[0],
                                drop: b[1],
                            });
                        }
                    }
                }
                publish(Snapshot {
                    epoch: sample.request.epoch,
                    at: sample.request.at,
                    eye: sample.request.eye,
                    regions: regions.into(),
                });
            }
        }
        self.staging.unmap();
    }
}
fn publish(snapshot: Snapshot) {
    if snapshot.epoch != epoch() {
        return;
    }
    if let Ok(mut exchange) = EXCHANGE.try_lock() {
        exchange.1 = Some(snapshot);
    }
}

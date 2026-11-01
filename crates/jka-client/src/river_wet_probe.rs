//! Debug-only, asynchronous readback of the *actual GPU result* at a live
//! riversource. It inspects a tiny 16x16 tile centred on the source, not the
//! complete river grid: the effective cost is independent of map size.
//! Nothing is allocated or copied unless r_riverDebug is set to 2.
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use crate::river_gpu::Simulation;
use jka_water::river::CENTIMETRES_PER_MAP_UNIT;

const SAMPLE_INTERVAL: Duration = Duration::from_millis(1500);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const TILE: u32 = 16;
const STRIDE: u64 = 16; // one vec4<f32> per surface/geometry cell

enum Readback {
    Idle,
    Copied,
    Mapping(Receiver<Result<(), wgpu::BufferAsyncError>>),
}

pub(super) struct WetSpotProbe {
    width: u32,
    height: u32,
    origin: [u32; 2],
    grid_width: u32,
    bytes_per_field: u64,
    staging: wgpu::Buffer,
    readback: Readback,
    last_sample: Instant,
    last_poll: Instant,
}

impl WetSpotProbe {
    pub(super) fn new(device: &wgpu::Device, dimensions: [u32; 2]) -> Self {
        let width = dimensions[0].min(TILE);
        let height = dimensions[1].min(TILE);
        let origin = [(dimensions[0] - width) / 2, (dimensions[1] - height) / 2];
        let bytes_per_field = u64::from(width) * u64::from(height) * STRIDE;
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("River debug source water/bed async readback"),
            size: bytes_per_field * 2,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        // Start eligible immediately; the first real solver step will provide
        // a valid surface after the existing PrepareSurface stage.
        let now = Instant::now();
        Self {
            width,
            height,
            origin,
            grid_width: dimensions[0],
            bytes_per_field,
            staging,
            readback: Readback::Idle,
            last_sample: now,
            last_poll: now,
        }
    }

    /// Called after the river's hydraulic and surface passes are encoded.
    pub(super) fn encode_sample(&mut self, encoder: &mut wgpu::CommandEncoder, sim: &Simulation) {
        if !matches!(&self.readback, Readback::Idle) || self.last_sample.elapsed() < SAMPLE_INTERVAL {
            return;
        }
        // Only the source footprint and a narrow band around it. COPY_BUFFER
        // requires 4-byte alignment; vec4 rows are 16-byte aligned already.
        for (field, source) in [sim.surface(), sim.bed_geometry()].into_iter().enumerate() {
            for y in 0..self.height {
                let src_offset = (u64::from(self.origin[1] + y) * u64::from(self.grid_width)
                    + u64::from(self.origin[0])) * STRIDE;
                let dest_offset = (field as u64) * self.bytes_per_field
                    + u64::from(y) * u64::from(self.width) * STRIDE;
                encoder.copy_buffer_to_buffer(
                    source,
                    src_offset,
                    &self.staging,
                    dest_offset,
                    u64::from(self.width) * STRIDE,
                );
            }
        }
        self.readback = Readback::Copied;
        self.last_sample = Instant::now();
    }

    pub(super) fn poll(&mut self, device: &wgpu::Device, origin_z_cm: f64, target_map_z: f32) {
        if matches!(&self.readback, Readback::Copied) {
            let (sender, receiver) = mpsc::channel();
            self.staging.slice(..).map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
            self.readback = Readback::Mapping(receiver);
        }
        if matches!(&self.readback, Readback::Mapping(_)) && self.last_poll.elapsed() >= POLL_INTERVAL {
            let _ = device.poll(wgpu::PollType::Poll); // Never wait for GPU completion.
            self.last_poll = Instant::now();
        }
        let result = match &self.readback {
            Readback::Mapping(receiver) => receiver.try_recv().ok(),
            _ => None,
        };
        let Some(result) = result else { return; };
        self.readback = Readback::Idle;
        if let Err(err) = result {
            eprintln!("[RIVER DBG GPU] async map failed: {err}");
            return;
        }
        let view = match self.staging.slice(..).get_mapped_range() {
            Ok(view) => view,
            Err(err) => {
                self.staging.unmap();
                eprintln!("[RIVER DBG GPU] mapped range failed: {err:?}");
                return;
            }
        };
        let sample = |field: usize, x: u32, y: u32, component: usize| -> f32 {
            let at = field * self.bytes_per_field as usize
                + ((y * self.width + x) as usize * STRIDE as usize)
                + component * 4;
            f32::from_le_bytes(view[at..at + 4].try_into().expect("one GPU f32"))
        };
        let mut source_wet = 0u32;
        let mut source_blocked = 0u32;
        let mut source_cells = 0u32;
        let mut around_wet = 0u32;
        let mut source_depth_max = 0.0f32;
        let mut source_depth_sum = 0.0f32;
        let mut bed_min = f32::INFINITY;
        let mut bed_max = f32::NEG_INFINITY;
        let mut invalid = 0u32;
        let half = (self.width / 2, self.height / 2);
        for y in 0..self.height {
            for x in 0..self.width {
                let depth = sample(0, x, y, 1);
                let bed_cm = sample(1, x, y, 0);
                if !depth.is_finite() || !bed_cm.is_finite() {
                    invalid += 1;
                    continue;
                }
                if depth > 0.1 { around_wet += 1; }
                // The core 8x8 region includes the complete 6x6 source
                // footprint at 128 cells / +/-1024 map units / width 96.
                if x >= half.0.saturating_sub(4) && x < half.0 + 4
                    && y >= half.1.saturating_sub(4) && y < half.1 + 4 {
                    let bed_map_z = ((origin_z_cm + f64::from(bed_cm))
                        / f64::from(CENTIMETRES_PER_MAP_UNIT)) as f32;
                    source_cells += 1;
                    bed_min = bed_min.min(bed_map_z);
                    bed_max = bed_max.max(bed_map_z);
                    if bed_map_z >= target_map_z - 0.1 { source_blocked += 1; }
                    if depth > 0.1 { source_wet += 1; }
                    source_depth_sum += depth.max(0.0);
                    source_depth_max = source_depth_max.max(depth);
                }
            }
        }
        drop(view);
        self.staging.unmap();
        // These values come from the GPU's reconstructed bed and prepared
        // surface, not CPU triangle probes or the submitted draw-call status.
        eprintln!("[RIVER DBG GPU] target_z={target_map_z:.1} source_wet={source_wet}/{source_cells} nearby_wet={around_wet}/{} source_bed_z=[{bed_min:.1},{bed_max:.1}] bed_above_target={source_blocked}/{source_cells} source_depth_mean={:.2}cm max={source_depth_max:.2}cm invalid={} ({})",
            self.width * self.height,
            source_depth_sum / (source_cells.max(1) as f32),
            invalid,
            if source_wet > 0 { "WATER EXISTS ON GPU: check draw/depth/visibility" }
            else if source_blocked > 0 { "SOURCE BED TOO HIGH: inspect collision capture/solid obstacles" }
            else { "SOURCE DRY: inspect source boundary / solver state" },
        );
    }
}

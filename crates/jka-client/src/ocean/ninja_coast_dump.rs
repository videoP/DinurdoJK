//! One-shot f32 readback of the Fluid Ninja Coast simulation targets
//! (`r_ninjaCoastDump`), for value-range comparison with the UE PIE dump
//! (`tools/coast/capture_coast.py`). Files use the same layout as the UE
//! capture: raw little-endian RGBA f32, row-major, row 0 = texel row 0
//! (field V = JKA +Y). Two-channel targets read back as (r, g, 0, 1).
//!
//! Debug only: nothing is allocated until a dump is requested, the copy is
//! encoded after the solver step of that frame, mapped a frame later without
//! a CPU wait, and written on a short-lived thread.
use std::sync::mpsc::{Receiver, TryRecvError};

const SHADER: &str = r"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> dst: array<vec4<f32>>;
@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(src);
    if id.x >= size.x || id.y >= size.y { return; }
    dst[id.y * size.x + id.x] = textureLoad(src, vec2<i32>(id.xy), 0);
}";

struct Target { name: &'static str, size: [u32; 2], offset: u64 }

enum State { Copied, Mapping(Receiver<Result<(), wgpu::BufferAsyncError>>) }

struct Pending { token: u32, targets: Vec<Target>, readback: wgpu::Buffer, state: State, meta: serde_json::Value }

#[derive(Default)]
pub(super) struct Dumper {
    pipeline: Option<(wgpu::BindGroupLayout, wgpu::ComputePipeline)>,
    /// Last `r_ninjaCoastDump` value seen. The first value is adopted without
    /// dumping, so an archived value does not dump at startup.
    seen: Option<u32>,
    request: Option<u32>,
    pending: Option<Pending>,
}

impl Dumper {
    /// Edge trigger: a change to a nonzero value requests one dump.
    pub fn set_token(&mut self, token: u32) {
        if self.seen.is_some_and(|seen| seen != token) && token != 0 { self.request = Some(token); }
        self.seen = Some(token);
    }

    pub fn wanted(&self) -> bool { self.request.is_some() && self.pending.is_none() }

    /// Copies `views` (any float-sampled 2D views) into one readback buffer.
    pub fn encode(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder,
        views: &[(&'static str, &wgpu::TextureView)], meta: serde_json::Value) {
        let Some(token) = self.request.take() else { return };
        let (layout, pipeline) = self.pipeline.get_or_insert_with(|| {
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("Ninja coast dump"),
                entries: &[super::ninja_ocean::texture(0, wgpu::ShaderStages::COMPUTE), super::ninja_ocean::storage(1, false)] });
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("Ninja coast dump"),
                bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("Ninja coast dump"),
                source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some("Ninja coast dump"),
                layout: Some(&pl), module: &module, entry_point: Some("main"), compilation_options: Default::default(), cache: None });
            (layout, pipeline)
        });
        let mut targets = Vec::with_capacity(views.len());
        let mut total = 0u64;
        for (name, view) in views {
            let texture = view.texture();
            let size = [texture.width(), texture.height()];
            targets.push(Target { name: *name, size, offset: total });
            total += u64::from(size[0]) * u64::from(size[1]) * 16;
        }
        if total == 0 { return; }
        let readback = super::ninja_ocean::buf(device, "Ninja coast dump readback", total,
            wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST);
        for ((_, view), target) in views.iter().zip(&targets) {
            let bytes = u64::from(target.size[0]) * u64::from(target.size[1]) * 16;
            let storage = super::ninja_ocean::buf(device, "Ninja coast dump texels", bytes,
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC);
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("Ninja coast dump"), layout,
                entries: &[super::ninja_ocean::t(0, view), super::ninja_ocean::b(1, &storage)] });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("Ninja coast dump"), timestamp_writes: None });
                pass.set_pipeline(pipeline); pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups(target.size[0].div_ceil(8), target.size[1].div_ceil(8), 1);
            }
            encoder.copy_buffer_to_buffer(&storage, 0, &readback, target.offset, bytes);
        }
        self.pending = Some(Pending { token, targets, readback, state: State::Copied, meta });
    }

    /// Call once per frame before encoding: maps the previous frame's copy
    /// and hands the finished data to a writer thread.
    pub fn poll(&mut self) {
        let Some(pending) = self.pending.as_mut() else { return };
        if matches!(pending.state, State::Copied) {
            let (send, receive) = std::sync::mpsc::channel();
            pending.readback.slice(..).map_async(wgpu::MapMode::Read, move |r| { let _ = send.send(r); });
            pending.state = State::Mapping(receive);
            return;
        }
        let State::Mapping(receive) = &pending.state else { return };
        let status = match receive.try_recv() {
            Err(TryRecvError::Empty) => return,
            Ok(status) => status.map_err(|e| e.to_string()),
            Err(TryRecvError::Disconnected) => Err("map callback dropped".into()),
        };
        let pending = self.pending.take().expect("pending dump");
        if let Err(e) = status { eprintln!("[OCEAN NINJA] Coast dump {} failed: {e}", pending.token); return; }
        let data: Vec<f32> = match pending.readback.slice(..).get_mapped_range() {
            Ok(view) => bytemuck::cast_slice::<u8, f32>(&view).to_vec(),
            Err(e) => { eprintln!("[OCEAN NINJA] Coast dump {} failed: {e}", pending.token); return; }
        };
        pending.readback.unmap();
        let Pending { token, targets, meta, .. } = pending;
        let _ = std::thread::Builder::new().name("ninja-coast-dump".into()).spawn(move || {
            if let Err(e) = write(token, &targets, &data, meta) { eprintln!("[OCEAN NINJA] Coast dump {token} failed: {e}"); }
        });
    }
}

#[derive(Default)]
struct Channels { min: [f32; 4], max: [f32; 4], sum: [f64; 4], finite: usize, nonfinite: usize }

fn channels(texels: &[[f32; 4]]) -> serde_json::Value {
    let mut c = Channels { min: [f32::INFINITY; 4], max: [f32::NEG_INFINITY; 4], ..Default::default() };
    for t in texels {
        if t.iter().any(|v| !v.is_finite()) { c.nonfinite += 1; continue; }
        c.finite += 1;
        for i in 0..4 { c.min[i] = c.min[i].min(t[i]); c.max[i] = c.max[i].max(t[i]); c.sum[i] += f64::from(t[i]); }
    }
    let n = c.finite.max(1) as f64;
    serde_json::json!({ "min": c.min, "max": c.max, "mean": c.sum.map(|s| s / n), "nonfinite": c.nonfinite })
}

fn percentile(sorted: &[f32], p: f64) -> f32 {
    if sorted.is_empty() { return 0.0; }
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

/// RT_VelocityDensity (rg velocity, b density, a wet): the same statistics
/// `tools/coast/compare_vd.py` prints for the UE dump.
fn velocity_stats(texels: &[[f32; 4]]) -> serde_json::Value {
    let mut speeds = Vec::new();
    let (mut sum, mut clamped, mut wet) = ([0.0f64; 2], 0usize, 0usize);
    for t in texels.iter().filter(|t| t.iter().all(|v| v.is_finite())) {
        if t[3] > 0.5 { wet += 1; }
        if t[2] <= 0.5 { continue; }
        sum[0] += f64::from(t[0]); sum[1] += f64::from(t[1]);
        if t[0].abs().max(t[1].abs()) >= 3.9 { clamped += 1; }
        speeds.push(t[0].hypot(t[1]));
    }
    speeds.sort_by(f32::total_cmp);
    let n = speeds.len().max(1) as f64;
    serde_json::json!({
        "dense_texels": speeds.len(), "dense_fraction": speeds.len() as f64 / texels.len().max(1) as f64,
        "wet_fraction": wet as f64 / texels.len().max(1) as f64,
        "mean_velocity": [sum[0] / n, sum[1] / n],
        "speed": { "mean": speeds.iter().map(|&s| f64::from(s)).sum::<f64>() / n,
            "p50": percentile(&speeds, 0.5), "p90": percentile(&speeds, 0.9), "p99": percentile(&speeds, 0.99),
            "max": speeds.last().copied().unwrap_or(0.0) },
        "clamped_fraction": clamped as f64 / n,
    })
}

fn write(token: u32, targets: &[Target], data: &[f32], mut meta: serde_json::Value) -> std::io::Result<()> {
    let root = jka_assets::pk3::installed_dinurdojk_directory().unwrap_or_else(|| std::path::PathBuf::from("DinurdoJK"));
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let dir = root.join("debug").join("ninja_coast").join(format!("dump_{token}_{stamp}"));
    std::fs::create_dir_all(&dir)?;
    let mut stats = serde_json::Map::new();
    let mut summary = String::new();
    for target in targets {
        let start = (target.offset / 4) as usize;
        let floats = &data[start..start + target.size[0] as usize * target.size[1] as usize * 4];
        let texels: &[[f32; 4]] = bytemuck::cast_slice(floats);
        std::fs::write(dir.join(format!("{}_{}x{}.f32", target.name, target.size[0], target.size[1])), bytemuck::cast_slice::<f32, u8>(floats))?;
        let mut entry = serde_json::json!({ "size": target.size, "channels": channels(texels) });
        if target.name == "RT_VelocityDensity" {
            let v = velocity_stats(texels);
            summary = format!("dense {:.3}, |v| mean {:.3} p90 {:.3} p99 {:.3} max {:.3}, clamped {:.4}, mean v {}",
                v["dense_fraction"].as_f64().unwrap_or(0.0), v["speed"]["mean"].as_f64().unwrap_or(0.0),
                v["speed"]["p90"].as_f64().unwrap_or(0.0), v["speed"]["p99"].as_f64().unwrap_or(0.0),
                v["speed"]["max"].as_f64().unwrap_or(0.0), v["clamped_fraction"].as_f64().unwrap_or(0.0), v["mean_velocity"]);
            entry["velocity"] = v;
        }
        stats.insert(target.name.to_string(), entry);
    }
    meta["targets"] = serde_json::Value::Object(stats);
    std::fs::write(dir.join("stats.json"), serde_json::to_string_pretty(&meta).unwrap_or_default())?;
    eprintln!("[OCEAN NINJA] Coast dump {token} -> {}: {summary}", dir.display());
    Ok(())
}

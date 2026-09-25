// Temporary diagnostic: runs ocean_compute.wgsl exactly as ocean.rs does and
// reads the displacement / normal-foam arrays back to the CPU.
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

const G: f32 = 9.81;
const CASCADES: usize = 3;
const DEPTH_METERS: f32 = 20.0;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CascadeGpu {
    tile: [f32; 4],
    spectrum: [f32; 4],
    shape: [f32; 4],
    foam: [f32; 4],
    seed: [u32; 4],
    time: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct OceanState {
    values: [u32; 4],
}

struct Cascade {
    tile: [f32; 2],
    wind_speed: f32,
    wind_direction: f32,
    fetch_length: f32,
    swell: f32,
    spread: f32,
    detail: f32,
    whitecap: f32,
    foam_amount: f32,
}

fn defaults() -> [Cascade; CASCADES] {
    [
        Cascade { tile: [88.0, 88.0], wind_speed: 10.0, wind_direction: 20.0, fetch_length: 150.0, swell: 0.8, spread: 0.2, detail: 1.0, whitecap: 0.5, foam_amount: 8.0 },
        Cascade { tile: [57.0, 57.0], wind_speed: 5.0, wind_direction: 15.0, fetch_length: 150.0, swell: 0.8, spread: 0.4, detail: 1.0, whitecap: 0.5, foam_amount: 0.0 },
        Cascade { tile: [16.0, 16.0], wind_speed: 20.0, wind_direction: 20.0, fetch_length: 550.0, swell: 0.8, spread: 0.4, detail: 1.0, whitecap: 0.25, foam_amount: 3.0 },
    ]
}

fn jonswap_alpha(w: f32, f: f32) -> f32 {
    0.076 * (w * w / (f * G)).powf(0.22)
}
fn jonswap_peak(w: f32, f: f32) -> f32 {
    22.0 * (G * G / (w * f)).powf(1.0 / 3.0)
}

fn params(times: &[f32; CASCADES], delta: f32) -> [CascadeGpu; CASCADES] {
    let cs = defaults();
    std::array::from_fn(|i| {
        let s = &cs[i];
        let fetch = s.fetch_length * 1000.0;
        CascadeGpu {
            tile: [s.tile[0], s.tile[1], [1.0,0.75,0.0][i], 0.0],
            spectrum: [
                jonswap_alpha(s.wind_speed, fetch),
                jonswap_peak(s.wind_speed, fetch),
                s.wind_speed,
                (s.wind_direction + 180.0).to_radians(),
            ],
            shape: [DEPTH_METERS, s.swell, s.detail, s.spread],
            foam: [s.whitecap, delta * s.foam_amount * 7.5, delta * (10.0 - s.foam_amount).max(0.5) * 1.15, 0.0],
            seed: [85619173,85619173 ^ (i as u32).wrapping_mul(0x9e3779b9),0,0],
            time: [times[i], 0.0, 0.0, 0.0],
        }
    })
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = ((bits >> 15) & 1) as u32;
    let exp = ((bits >> 10) & 0x1f) as u32;
    let frac = (bits & 0x3ff) as u32;
    let v = if exp == 0 {
        if frac == 0 { 0.0 } else { (frac as f32) * 2f32.powi(-24) }
    } else if exp == 31 {
        if frac == 0 { f32::INFINITY } else { f32::NAN }
    } else {
        (1.0 + frac as f32 / 1024.0) * 2f32.powi(exp as i32 - 15)
    };
    if sign == 1 { -v } else { v }
}

fn stats(name: &str, v: &[f32]) {
    let finite: Vec<f32> = v.iter().copied().filter(|x| x.is_finite()).collect();
    let nonfinite = v.len() - finite.len();
    if finite.is_empty() {
        println!("    {name:<10} ALL NON-FINITE ({} values)", v.len());
        return;
    }
    let mean = finite.iter().sum::<f32>() / finite.len() as f32;
    let var = finite.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / finite.len() as f32;
    let min = finite.iter().copied().fold(f32::INFINITY, f32::min);
    let max = finite.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    println!(
        "    {name:<10} rms={:<12.5} min={:<12.4} max={:<12.4} mean={:<12.5} nonfinite={nonfinite}",
        var.sqrt(), min, max, mean
    );
}

fn main() {
    let map_size: u32 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(1024);
    let frames: usize = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(40);
    futures_lite::future::block_on(run(map_size, frames));
}

async fn run(map_size: u32, frames: usize) {
    let instance = wgpu::Instance::default();
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .expect("adapter");
    println!("adapter: {:?}", adapter.get_info());
    let lim = adapter.limits();
    println!("  max_compute_invocations_per_workgroup = {}", lim.max_compute_invocations_per_workgroup);
    println!("  max_compute_workgroup_size_x = {}", lim.max_compute_workgroup_size_x);
    println!("  max_compute_workgroup_storage_size = {}", lim.max_compute_workgroup_storage_size);
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: None,
            required_features: wgpu::Features::empty(),
            required_limits: lim.clone(),
            memory_hints: Default::default(),
            trace: wgpu::Trace::Off,
            experimental_features: Default::default(),
        })
        .await
        .expect("device");
    device.on_uncaptured_error(std::sync::Arc::new(|e| eprintln!("wgpu error: {e}")));

    // Validate the world shader too: naga runs on create_shader_module, so a
    // WGSL error in the ocean render path surfaces here instead of at runtime.
    {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let _ = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("bsp.wgsl"),
            source: wgpu::ShaderSource::Wgsl(format!("{}
{}", include_str!("../src/bsp.wgsl"), include_str!("../src/surface_deformation.wgsl")).into()),
        });
        match futures_lite::future::block_on(scope.pop()) {
            Some(e) => println!("bsp.wgsl VALIDATION ERROR:
{e}"),
            None => println!("bsp.wgsl parsed and validated OK"),
        }
    }

    let stages = map_size.ilog2();
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ocean compute"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../src/ocean_compute.wgsl").into()),
    });
    let storage = |label: &str, size: u64| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    let n = u64::from(map_size);
    let params_size = (std::mem::size_of::<CascadeGpu>() * CASCADES) as u64;
    let params_buffer = storage("params", params_size);
    let spectrum_buffer = storage("spectrum", n * n * CASCADES as u64 * 16);
    let butterfly_buffer = storage("butterfly", n * u64::from(stages) * 16);
    let fft_buffer = storage("fft", n * n * 4 * 2 * 8);
    let foam_buffer = storage("foam", n * n * CASCADES as u64 * 4);

    let tex = |label: &'static str| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: map_size, height: map_size, depth_or_array_layers: CASCADES as u32 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    };
    let displacement = tex("displacement");
    let normal = tex("normal");
    let dv = displacement.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let nv = normal.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });

    let se = |b: u32, ro: bool| wgpu::BindGroupLayoutEntry {
        binding: b,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: ro }, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    };
    let ste = |b: u32| wgpu::BindGroupLayoutEntry {
        binding: b,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly, format: wgpu::TextureFormat::Rgba16Float, view_dimension: wgpu::TextureViewDimension::D2Array },
        count: None,
    };
    let ue = |b: u32| wgpu::BindGroupLayoutEntry {
        binding: b,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[se(0, true), se(1, false), se(2, false), se(3, false), se(4, false), ste(5), ste(6), ue(7)],
    });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let cp = |name: &str| {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(name),
            layout: Some(&pl),
            module: &shader,
            entry_point: Some(name),
            compilation_options: Default::default(),
            cache: None,
        })
    };
    let p_spectrum = cp("spectrum_compute");
    let p_butterfly = cp("fft_butterfly");
    let p_modulate = cp("spectrum_modulate");
    let p_fft = cp("fft_compute");
    let p_transpose = cp("transpose");
    let p_unpack = cp("fft_unpack");
    println!("all pipelines created OK");

    let binds: Vec<_> = (0..CASCADES)
        .map(|c| {
            let state = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::bytes_of(&OceanState { values: [c as u32, map_size, CASCADES as u32, 0] }),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: params_buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: spectrum_buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: butterfly_buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: fft_buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 4, resource: foam_buffer.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&dv) },
                    wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(&nv) },
                    wgpu::BindGroupEntry { binding: 7, resource: state.as_entire_binding() },
                ],
            })
        })
        .collect();

    let mut times: [f32; CASCADES] = [120.0, 120.0 + std::f32::consts::PI, 120.0 + 2.0 * std::f32::consts::PI];
    queue.write_buffer(&params_buffer, 0, bytemuck::cast_slice(&params(&times, 0.0)));

    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut p = enc.begin_compute_pass(&Default::default());
        p.set_pipeline(&p_butterfly);
        p.set_bind_group(0, &binds[0], &[]);
        p.dispatch_workgroups(map_size / 128, stages, 1);
    }
    queue.submit(Some(enc.finish()));

    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut p = enc.begin_compute_pass(&Default::default());
        p.set_pipeline(&p_spectrum);
        p.set_bind_group(0, &binds[0], &[]);
        p.dispatch_workgroups(map_size / 16, map_size / 16, CASCADES as u32);
    }
    queue.submit(Some(enc.finish()));
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();

    let delta = 1.0 / 50.0f32;
    for f in 0..frames {
        for t in times.iter_mut() {
            *t += delta;
        }
        queue.write_buffer(&params_buffer, 0, bytemuck::cast_slice(&params(&times, delta)));
        let mut enc = device.create_command_encoder(&Default::default());
        for c in 0..CASCADES {
            let b = &binds[c];
            {
                let mut p = enc.begin_compute_pass(&Default::default());
                p.set_bind_group(0, b, &[]);
                p.set_pipeline(&p_modulate);
                p.dispatch_workgroups(map_size / 16, map_size / 16, 1);
            }
            {
                let mut p = enc.begin_compute_pass(&Default::default());
                p.set_bind_group(0, b, &[]);
                p.set_pipeline(&p_fft);
                p.dispatch_workgroups(1, map_size, 4);
            }
            {
                let mut p = enc.begin_compute_pass(&Default::default());
                p.set_bind_group(0, b, &[]);
                p.set_pipeline(&p_transpose);
                p.dispatch_workgroups(map_size / 32, map_size / 32, 4);
            }
            {
                let mut p = enc.begin_compute_pass(&Default::default());
                p.set_bind_group(0, b, &[]);
                p.set_pipeline(&p_fft);
                p.dispatch_workgroups(1, map_size, 4);
            }
            {
                let mut p = enc.begin_compute_pass(&Default::default());
                p.set_bind_group(0, b, &[]);
                p.set_pipeline(&p_unpack);
                p.dispatch_workgroups(map_size / 16, map_size / 16, 1);
            }
        }
        queue.submit(Some(enc.finish()));
        if f % 10 == 0 {
            device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
        }
    }
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();

    let bpr = map_size * 8;
    let padded = bpr.div_ceil(256) * 256;
    let size = u64::from(padded) * u64::from(map_size) * CASCADES as u64;
    let read = |t: &wgpu::Texture, label: &str| -> Vec<f32> {
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(map_size) },
            },
            wgpu::Extent3d { width: map_size, height: map_size, depth_or_array_layers: CASCADES as u32 },
        );
        queue.submit(Some(enc.finish()));
        buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
        let data = buf.slice(..).get_mapped_range().to_vec();
        let mut out = Vec::with_capacity((map_size * map_size * 4 * CASCADES as u32) as usize);
        for layer in 0..CASCADES {
            for y in 0..map_size {
                let off = (layer as u64 * u64::from(padded) * u64::from(map_size) + u64::from(y) * u64::from(padded)) as usize;
                for x in 0..map_size as usize {
                    for ch in 0..4 {
                        let i = off + x * 8 + ch * 2;
                        out.push(f16_to_f32(u16::from_le_bytes([data[i], data[i + 1]])));
                    }
                }
            }
        }
        out
    };
    // --- Replicate bsp.wgsl's sample_ocean_displacement() against the live
    // --- displacement array, using the same uniform layout and sampler.
    {
        #[repr(C)]
        #[derive(Clone, Copy, Pod, Zeroable)]
        struct RenderUniform {
            map_scales: [[f32; 4]; CASCADES],
            water_color: [f32; 4],
            foam_color: [f32; 4],
            ocean_info: [f32; 4],
            surface: [f32; 4],
        }
        let cs = defaults();
        let dscale = [1.0f32, 0.75, 0.0];
        let nscale = [1.0f32, 1.0, 0.25];
        let uniform = RenderUniform {
            map_scales: std::array::from_fn(|i| {
                [1.0 / cs[i].tile[0], 1.0 / cs[i].tile[1], dscale[i], nscale[i]]
            }),
            water_color: [0.0100, 0.0194, 0.0273, 1.0],
            foam_color: [0.4910, 0.4072, 0.3435, 1.0],
            ocean_info: [1.0, map_size as f32, 40.0, CASCADES as f32],
            surface: [0.65, 1.0, 0.0, 0.0],
        };
        let ubuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vs probe uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let probe_params: [f32; 4] = [0.0, 0.0, 10.0, 0.0]; // 128x128 samples, 10-unit spacing
        let pbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vs probe params"),
            contents: bytemuck::bytes_of(&probe_params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let obuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vs probe out"),
            size: 128 * 128 * 16,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let rbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("vs probe readback"),
            size: 128 * 128 * 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("ocean repeat sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let sh = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vs probe"),
            source: wgpu::ShaderSource::Wgsl(include_str!("vs_probe.wgsl").into()),
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let sampled_view = displacement.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&sampled_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: ubuf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: obuf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: pbuf.as_entire_binding() },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&bgl)], immediate_size: 0 });
        let pipe = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: None, layout: Some(&pl), module: &sh, entry_point: Some("main"), compilation_options: Default::default(), cache: None });
        let mut enc = device.create_command_encoder(&Default::default());
        { let mut p = enc.begin_compute_pass(&Default::default()); p.set_pipeline(&pipe); p.set_bind_group(0, &bg, &[]); p.dispatch_workgroups(16, 16, 1); }
        enc.copy_buffer_to_buffer(&obuf, 0, &rbuf, 0, 128 * 128 * 16);
        queue.submit(Some(enc.finish()));
        rbuf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
        let raw = rbuf.slice(..).get_mapped_range().to_vec();
        let vals: &[f32] = bytemuck::cast_slice(&raw);
        let ch = |k: usize| vals.iter().skip(k).step_by(4).copied().collect::<Vec<f32>>();
        println!("\nVERTEX-PATH REPLICA (128x128 samples over 1280 world units, 10u spacing):");
        stats("disp.x m", &ch(0));
        stats("disp.y m", &ch(1));
        stats("disp.z m", &ch(2));
        let y = ch(1);
        let units: Vec<f32> = y.iter().map(|v| v * 40.0).collect();
        stats("disp.y u", &units);
        println!("    map_scales[0].z (displacement_scale) read back as {}", ch(3)[0]);
    }

    let disp = read(&displacement, "disp-read");
    let norm = read(&normal, "norm-read");
    let per = (map_size * map_size * 4) as usize;
    for c in 0..CASCADES {
        println!("cascade {c}:");
        let d = &disp[c * per..(c + 1) * per];
        let nn = &norm[c * per..(c + 1) * per];
        let ch = |s: &[f32], k: usize| s.iter().skip(k).step_by(4).copied().collect::<Vec<f32>>();
        stats("disp.x", &ch(d, 0));
        stats("disp.y(h)", &ch(d, 1));
        stats("disp.z", &ch(d, 2));
        stats("grad.x", &ch(nn, 0));
        stats("grad.y", &ch(nn, 1));
        stats("dhx_dx", &ch(nn, 2));
        let foam = ch(nn, 3);
        stats("foam", &foam);
        let hi = foam.iter().filter(|v| **v > 0.9).count() as f64 / foam.len() as f64 * 100.0;
        let zero = foam.iter().filter(|v| **v <= 0.001).count() as f64 / foam.len() as f64 * 100.0;
        println!("    foam>0.9: {hi:.1}%   foam~0: {zero:.1}%");
    }
}

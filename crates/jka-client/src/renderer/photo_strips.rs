//! GPU reprojection from a resident cube. Direct output renders one image;
//! strip output renders bounded row bands (split into texture-limit segments
//! when wider), reads them back through a 3-slot ring and streams them to the
//! encoder. No full-output texture, readback buffer or RAM image in strips.
use super::{check_cancel, plan::{CapturePlan, READBACK_SLOTS}, readback::ReadbackRing, stream, PhotoCaptureRequest, PhotoPerf, Renderer, UserEvent};
use crate::runtime::ScreenshotOutput;
use rayon::prelude::*;
use winit::event_loop::EventLoopProxy;

const PARAMS_SIZE: u64 = 32;

pub(super) struct Projection {
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    params: wgpu::Buffer,
    stride: u64,
}
impl Projection {
    /// One pipeline per capture job. `regions` parameter slots; a region must
    /// not be rewritten until every submission that reads it has completed.
    pub(super) fn new(device: &wgpu::Device, cube: &wgpu::Texture, samples: u32, regions: u32) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Photo projection"),
            source: wgpu::ShaderSource::Wgsl(include_str!("photo_projection.wgsl").replace("SAMPLES_VALUE", &format!("{samples}u")).into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Photo projection"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, count: None,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::Cube, multisampled: false } },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, count: None,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering) },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, count: None,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: std::num::NonZeroU64::new(PARAMS_SIZE) } },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Photo projection"), bind_group_layouts: &[Some(&layout)], immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Photo projection"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"), buffers: &[], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState { format: cube.format(), blend: None, write_mask: wgpu::ColorWrites::ALL })],
                compilation_options: Default::default() }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let stride = PARAMS_SIZE.next_multiple_of(u64::from(device.limits().min_uniform_buffer_offset_alignment.max(1)));
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Photo projection params"), size: stride * u64::from(regions.max(1)),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
        });
        let cube_view = cube.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::Cube), ..Default::default() });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Photo projection cube"), layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&cube_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &params, offset: 0, size: std::num::NonZeroU64::new(PARAMS_SIZE) }) },
            ],
        });
        Self { pipeline, bind, params, stride }
    }

    /// Staged by the queue and applied at the next submit, in queue order.
    pub(super) fn write(&self, queue: &wgpu::Queue, region: u32, origin: [u32; 2], full: [u32; 2], mode: u32) {
        let mut bytes = [0u8; PARAMS_SIZE as usize];
        for (i, v) in [origin[0] as f32, origin[1] as f32, full[0] as f32, full[1] as f32].into_iter().enumerate() {
            bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        bytes[16..20].copy_from_slice(&mode.to_le_bytes());
        queue.write_buffer(&self.params, self.stride * u64::from(region), &bytes);
    }

    pub(super) fn draw(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, region: u32, size: [u32; 2]) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Photo reprojection"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, depth_slice: None, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[(self.stride * u64::from(region)) as u32]);
        pass.set_scissor_rect(0, 0, size[0], size[1]);
        pass.draw(0..3, 0..1);
    }
}

/// Whole-image projection for the direct path (existing readback/encoder).
pub(super) fn project_full(source: &Renderer, projection: &Projection, format: wgpu::TextureFormat, size: [u32; 2], mode: u32) -> wgpu::Texture {
    let output = super::texture(&source.device, format, size, 1);
    let view = output.create_view(&Default::default());
    projection.write(&source.queue, 0, [0, 0], size, mode);
    let mut encoder = source.device.create_command_encoder(&Default::default());
    projection.draw(&mut encoder, &view, 0, size);
    source.queue.submit([encoder.finish()]);
    output
}

/// Tight RGB8 from padded RGBA/BGRA rows (parallel over rows).
fn unpack(src: &[u8], dst: &mut [u8], width: u32, padded: u32, bgra: bool) {
    let (width, padded) = (width as usize, padded as usize);
    dst.par_chunks_mut(width * 3).enumerate().for_each(|(y, row)| {
        let src = &src[y * padded..y * padded + width * 4];
        for (d, s) in row.chunks_exact_mut(3).zip(src.chunks_exact(4)) {
            if bgra { d.copy_from_slice(&[s[2], s[1], s[0]]); } else { d.copy_from_slice(&s[..3]); }
        }
    });
}

pub(super) fn stem(request: &PhotoCaptureRequest, name: &str) -> (String, &'static str) {
    (format!("shot{}_{}_{name}", super::super::capture::screenshot_timestamp(), request.metadata.captured_unix_ms),
     if request.options.format == super::ScreenshotFormat::Jpeg { "jpg" } else { "png" })
}

/// Finish a streamed output: private `.part` file -> final name in the
/// private capture directory (published later by `commit_photo_outputs`).
pub(super) fn finish_part(part: std::path::PathBuf, path: std::path::PathBuf) -> Result<ScreenshotOutput, String> {
    if path.exists() { return Err("Screenshot destination already exists".into()); }
    std::fs::rename(&part, &path).map_err(|e| format!("Could not finish panorama: {e}"))?;
    Ok(ScreenshotOutput::Saved(path))
}

pub(super) fn encode_strips(
    source: &Renderer, cube: &wgpu::Texture, plan: &CapturePlan, request: &PhotoCaptureRequest,
    mode: u32, name: &str, proxy: &EventLoopProxy<UserEvent>, perf: &mut PhotoPerf,
) -> Result<ScreenshotOutput, String> {
    let strip = plan.strip.ok_or("Missing strip plan")?;
    let [width, height] = plan.image;
    let device = &source.device;
    let format = cube.format();
    let bgra = matches!(format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
    let projection = Projection::new(device, cube, request.options.supersample, READBACK_SLOTS * strip.segments);
    let target = super::texture(device, format, [strip.segment, strip.rows], 1);
    let view = target.create_view(&Default::default());
    let mut ring = ReadbackRing::<(u32, u32)>::new(device, READBACK_SLOTS, u64::from(strip.padded_row) * u64::from(strip.rows), "Photo strip readback");
    perf.peak_gpu_buffers = perf.peak_gpu_buffers.max(ring.bytes() + u64::from(strip.segment) * u64::from(strip.rows) * 4);
    let equirect = request.options.projection == super::PhotoProjection::Equirectangular;
    let xmp = crate::screenshot::streaming_xmp(&request.metadata, equirect.then_some([width, height]))?;
    let (stem, extension) = stem(request, name);
    let part = request.directory.join(format!("{stem}.{extension}.part"));
    let job = stream::EncodeJob { path: part.clone(), jpeg: extension == "jpg", width, height, xmp,
        cancel: std::sync::Arc::clone(&request.cancel), proxy: proxy.clone() };
    let started = std::time::Instant::now();
    let (stats, send_wait) = stream::run(job, |sender| {
        let mut consume = |(_, rows): (u32, u32), bytes: &[u8]| -> Result<(), String> {
            let mut rgb = sender.buffer(rows);
            unpack(bytes, &mut rgb, width, strip.padded_row, bgra);
            sender.send(stream::Band { rows, rgb }).map_err(|_| stream::ENCODER_STOPPED.to_owned())
        };
        for y0 in (0..height).step_by(strip.rows as usize) {
            check_cancel(&request.cancel)?;
            let rows = strip.rows.min(height - y0);
            let slot = ring.acquire(device, &mut consume)?;
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Photo strip") });
            for segment in 0..strip.segments {
                let x0 = segment * strip.segment;
                let w = strip.segment.min(width - x0);
                let region = slot as u32 * strip.segments + segment;
                projection.write(&source.queue, region, [x0, y0], [width, height], mode);
                projection.draw(&mut encoder, &view, region, [w, rows]);
                encoder.copy_texture_to_buffer(target.as_image_copy(),
                    wgpu::TexelCopyBufferInfo { buffer: ring.buffer(slot), layout: wgpu::TexelCopyBufferLayout {
                        offset: u64::from(x0) * 4, bytes_per_row: Some(strip.padded_row), rows_per_image: Some(rows) } },
                    wgpu::Extent3d { width: w, height: rows, depth_or_array_layers: 1 });
            }
            let submission = source.queue.submit([encoder.finish()]);
            ring.submitted(slot, (y0, rows), submission);
            ring.poll(device, &mut consume)?;
        }
        ring.drain(device, &mut consume)
    })?;
    perf.projection += started.elapsed();
    perf.readback_wait += ring.wait;
    perf.unpack += ring.consume;
    perf.strips += ring.completed;
    perf.add_encode(stats, send_wait);
    finish_part(part, request.directory.join(format!("{stem}.{extension}")))
}

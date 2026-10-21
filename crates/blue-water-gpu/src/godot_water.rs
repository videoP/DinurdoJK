//! godot-water-shader's GPU resources for `r_oceanType 5` (vendor/godot-water-shader, MIT, Emil Jerkovic): the packed
//! pattern texture (its foam texture and noise textures, baked by the host) and the persistent foam simulation
//! (`godot_water_foam.wgsl`) of FoamScene's near (1024², 15 m) and far (512², 30 m) cascades around the camera.
//! Upstream's capture and history viewports ping-pong; here each cascade writes a stable output the surface samples and
//! copies it into its history afterwards, so the surface's bind groups never change from frame to frame.

use crate::original_surface::GodotWaterViews;
use bytemuck::{Pod, Zeroable};

/// Edge of the packed pattern texture (sea_foam.png's size).
pub const PATTERN_SIZE: u32 = 1024;
/// FoamScene's cascades: resolution and capture coverage (upstream metres).
pub const NEAR_CASCADE: (u32, f32) = (1024, 15.0);
pub const FAR_CASCADE: (u32, f32) = (512, 30.0);
// FoamSubviewport's `decay_rate` and `erosion_multiplier`, and foam_calc's `noise_intensity_x/z`, as FoamScene sets them.
const DECAY_RATE: f32 = 0.2;
const EROSION_MULTIPLIER: f32 = 3.0;
const NOISE_INTENSITY: [f32; 2] = [0.06, -0.06];
// Upstream's history viewports are HDR (16-bit float).
const FOAM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rg16Float;

/// The packed pattern texture: r sea_foam.png, g the seamless cellular noise, b the seamless smooth simplex noise, with
/// box-filtered mips, and its repeating sampler.
pub struct GodotPatterns {
    pub view: wgpu::TextureView,
    pub repeat: wgpu::Sampler,
}

impl GodotPatterns {
    /// `rgba`: `PATTERN_SIZE`² texels, row-major.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, rgba: &[u8]) -> Self {
        assert_eq!(rgba.len(), (PATTERN_SIZE * PATTERN_SIZE * 4) as usize, "Godot water pattern size");
        let levels = PATTERN_SIZE.ilog2() + 1;
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Godot water patterns"),
            size: wgpu::Extent3d { width: PATTERN_SIZE, height: PATTERN_SIZE, depth_or_array_layers: 1 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut level = rgba.to_vec();
        let mut size = PATTERN_SIZE;
        for mip in 0..levels {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: mip, origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All },
                &level,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(size * 4), rows_per_image: Some(size) },
                wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            );
            if size == 1 { break; }
            let half = size / 2;
            let at = |x: u32, y: u32, c: usize| u32::from(level[((y * size + x) * 4) as usize + c]);
            level = (0..half * half * 4).map(|i| {
                let (texel, c) = (i / 4, (i % 4) as usize);
                let (x, y) = (texel % half * 2, texel / half * 2);
                ((at(x, y, c) + at(x + 1, y, c) + at(x, y + 1, c) + at(x + 1, y + 1, c) + 2) / 4) as u8
            }).collect();
            size = half;
        }
        let repeat = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Godot water patterns"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        Self { view: texture.create_view(&Default::default()), repeat }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
struct FoamPass {
    capture: [f32; 4],
    history: [f32; 4],
    params: [f32; 4],
    contact_frame: [f32; 4],
    contact: [f32; 4],
}

/// A static contact field for the intersection foam: per texel the highest and lowest solid within the band (m,
/// relative to the field's level), row-major with x along grid x and rows along grid z; the first texel's centre at
/// `origin` (grid m), texels `cell` metres apart.
pub struct ContactField<'a> {
    pub texels: &'a [[f32; 2]],
    pub size: [u32; 2],
    pub origin: [f32; 2],
    pub cell: f32,
}

/// One frame of the foam simulation.
pub struct FoamFrame {
    /// The camera's grid xz (m).
    pub camera_xz: [f32; 2],
    /// The wave clock (s), as the surface's `jka_hybrid.godot.z`.
    pub seconds: f32,
    /// Seconds since the previous frame.
    pub delta: f32,
    pub scale: f32,
    /// Near and far cascades on (`r_gerstnerFoam` bits).
    pub enabled: [bool; 2],
    /// Simulation steps per second for each cascade (`r_gerstnerFoamRate`); 0 steps every frame, as upstream. Capped
    /// cascades are staggered so the two rarely step in the same frame.
    pub rate: f32,
    /// The water plane above the contact field's level (m); `None` leaves out the intersection foam.
    pub plane_above_field: Option<f32>,
}

struct Cascade {
    size: u32,
    coverage: f32,
    output: wgpu::Texture,
    output_view: wgpu::TextureView,
    history: wgpu::Texture,
    history_view: wgpu::TextureView,
    uniform: wgpu::Buffer,
    group: Option<wgpu::BindGroup>,
    /// The previous capture's centre (grid m) and scale, while its history is live.
    previous: Option<([f32; 2], f32)>,
    /// Seconds since this cascade last stepped (its next step's decay delta).
    waited: f32,
}

pub struct GodotFoam {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    clamp: wgpu::Sampler,
    patterns: wgpu::TextureView,
    repeat: wgpu::Sampler,
    contact: wgpu::TextureView,
    contact_frame: Option<[f32; 4]>,
    cascades: [Cascade; 2],
}

impl GodotFoam {
    pub fn new(device: &wgpu::Device, patterns: &GodotPatterns) -> Self {
        let source = format!("{}\n{}", include_str!("godot_water_foam.wgsl"), include_str!("godot_water.wgsl"));
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Godot water persistent foam"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let texture = |binding| crate::original_surface::sampled(binding, fragment, wgpu::TextureViewDimension::D2, false);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Godot water persistent foam"),
            entries: &[
                crate::original_surface::uniform(0, fragment),
                texture(1),
                texture(2),
                crate::original_surface::sampler(3, fragment, false),
                crate::original_surface::sampler(4, fragment, false),
                texture(5),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Godot water persistent foam"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Godot water persistent foam"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vertex"),
                compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fragment"),
                compilation_options: Default::default(), targets: &[Some(FOAM_FORMAT.into())] }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let clamp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Godot water foam clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let cascade = |(size, coverage): (u32, f32), label: &'static str| {
            let target = |usage, label| device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FOAM_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | usage,
                view_formats: &[],
            });
            let output = target(wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, label);
            let history = target(wgpu::TextureUsages::COPY_DST, "Godot water foam history");
            Cascade {
                size,
                coverage,
                output_view: output.create_view(&Default::default()),
                history_view: history.create_view(&Default::default()),
                output,
                history,
                uniform: crate::original_surface::buffer(device, "Godot water foam pass",
                    std::mem::size_of::<FoamPass>() as u64, false),
                group: None,
                previous: None,
                waited: 0.0,
            }
        };
        let mut foam = Self {
            pipeline,
            layout,
            clamp,
            patterns: patterns.view.clone(),
            repeat: patterns.repeat.clone(),
            contact: crate::original_surface::placeholder(device, "Godot water blank contact field", FOAM_FORMAT,
                wgpu::TextureViewDimension::D2, 1),
            contact_frame: None,
            cascades: [cascade(NEAR_CASCADE, "Godot water near foam"), cascade(FAR_CASCADE, "Godot water far foam")],
        };
        foam.rebuild_groups(device);
        foam
    }

    fn rebuild_groups(&mut self, device: &wgpu::Device) {
        use crate::original_surface::{entry_buffer, entry_sampler, entry_texture};
        for cascade in &mut self.cascades {
            cascade.group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Godot water persistent foam"),
                layout: &self.layout,
                entries: &[
                    entry_buffer(0, &cascade.uniform),
                    entry_texture(1, &cascade.history_view),
                    entry_texture(2, &self.patterns),
                    entry_sampler(3, &self.repeat),
                    entry_sampler(4, &self.clamp),
                    entry_texture(5, &self.contact),
                ],
            }));
        }
    }

    /// Upload the intersection foam's contact field (`None` leaves the intersection foam out).
    pub fn set_contact(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, field: Option<ContactField<'_>>) {
        match field {
            Some(field) if field.size[0] > 0 && field.size[1] > 0
                && field.texels.len() == (field.size[0] * field.size[1]) as usize => {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Godot water contact field"),
                    size: wgpu::Extent3d { width: field.size[0], height: field.size[1], depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: FOAM_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let halves: Vec<u16> = field.texels.iter().flatten().map(|v| half::f16::from_f32(*v).to_bits()).collect();
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: 0, origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All },
                    bytemuck::cast_slice(&halves),
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(field.size[0] * 4),
                        rows_per_image: Some(field.size[1]) },
                    wgpu::Extent3d { width: field.size[0], height: field.size[1], depth_or_array_layers: 1 },
                );
                self.contact = texture.create_view(&Default::default());
                let corner = field.origin.map(|v| v - 0.5 * field.cell);
                self.contact_frame = Some([corner[0], corner[1],
                    1.0 / (field.size[0] as f32 * field.cell), 1.0 / (field.size[1] as f32 * field.cell)]);
            }
            _ => {
                if self.contact_frame.is_none() { return; }
                self.contact = crate::original_surface::placeholder(device, "Godot water blank contact field",
                    FOAM_FORMAT, wgpu::TextureViewDimension::D2, 1);
                self.contact_frame = None;
            }
        }
        self.rebuild_groups(device);
    }

    /// Advance the enabled cascades that are due. Returns the surface's foam frame: each cascade's capture centre,
    /// [near x, z, far x, z] (grid m); a cascade that did not step keeps the centre its map was drawn at.
    pub fn update(&mut self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, frame: &FoamFrame) -> [f32; 4] {
        let scale = frame.scale.max(1e-3);
        // main.gd's capture centre: the camera snapped to whole upstream metres.
        let centre = frame.camera_xz.map(|v| (v / scale).round() * scale);
        let contact = match (self.contact_frame, frame.plane_above_field) {
            (Some(contact_frame), Some(above)) => (contact_frame, [1.0, above]),
            _ => ([0.0; 4], [0.0; 2]),
        };
        let period = if frame.rate > 0.0 { 1.0 / frame.rate } else { 0.0 };
        for cascade in &mut self.cascades {
            cascade.waited += frame.delta.max(0.0);
        }
        // Capped: the most overdue cascade steps; the other waits for a later frame unless it has fallen a whole
        // period behind (frame rates under twice the step rate), so the two land on different frames.
        let most_overdue = (0..2).filter(|&i| frame.enabled[i])
            .max_by(|&a, &b| self.cascades[a].waited.total_cmp(&self.cascades[b].waited));
        for (index, (cascade, enabled)) in self.cascades.iter_mut().zip(frame.enabled).enumerate() {
            if !enabled {
                cascade.previous = None;
                cascade.waited = 0.0;
                continue;
            }
            let fresh = cascade.previous.is_none_or(|(_, previous_scale)| previous_scale != scale);
            let due = period <= 0.0 || fresh || cascade.waited >= 2.0 * period
                || (cascade.waited >= period && most_overdue == Some(index));
            if !due {
                continue;
            }
            let coverage = cascade.coverage * scale;
            let (history_offset, valid) = match cascade.previous {
                Some((previous, previous_scale)) if previous_scale == scale => (
                    [(centre[0] - previous[0]) / coverage, (centre[1] - previous[1]) / coverage], 1.0),
                _ => ([0.0; 2], 0.0),
            };
            let pass = FoamPass {
                capture: [centre[0], centre[1], coverage, 1.0 / cascade.size as f32],
                history: [history_offset[0], history_offset[1], cascade.waited, frame.seconds],
                params: [scale, DECAY_RATE, EROSION_MULTIPLIER, valid],
                contact_frame: contact.0,
                contact: [contact.1[0], contact.1[1], NOISE_INTENSITY[0], NOISE_INTENSITY[1]],
            };
            queue.write_buffer(&cascade.uniform, 0, bytemuck::bytes_of(&pass));
            {
                let mut render = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Godot water persistent foam"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &cascade.output_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                });
                render.set_pipeline(&self.pipeline);
                render.set_bind_group(0, cascade.group.as_ref(), &[]);
                render.draw(0..3, 0..1);
            }
            encoder.copy_texture_to_texture(cascade.output.as_image_copy(), cascade.history.as_image_copy(),
                wgpu::Extent3d { width: cascade.size, height: cascade.size, depth_or_array_layers: 1 });
            cascade.previous = Some((centre, scale));
            cascade.waited = 0.0;
        }
        let drawn = |cascade: &Cascade| cascade.previous.map_or(centre, |(drawn, _)| drawn);
        let (near, far) = (drawn(&self.cascades[0]), drawn(&self.cascades[1]));
        [near[0], near[1], far[0], far[1]]
    }

    /// The surface's bindings: the patterns and the two cascades' outputs.
    pub fn views(&self) -> GodotWaterViews {
        GodotWaterViews {
            patterns: self.patterns.clone(),
            repeat: self.repeat.clone(),
            foam_near: self.cascades[0].output_view.clone(),
            foam_far: self.cascades[1].output_view.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::original_surface::{OriginalClipmap, OriginalSurfaceBindings, OriginalSurfacePipeline, SurfaceAppearance,
        SurfaceCamera};
    use crate::surface_waves::SurfaceWaves;
    use bytemuck::Zeroable;

    /// `r_oceanType 5` on the GPU: its pipelines in every pixel-detail mode, two frames of persistent foam against a
    /// contact field, and a surface draw with its textures bound. The foam must form (Jacobian foam somewhere in the
    /// near capture, intersection foam where the field puts a solid through the water) and stay within 0..1.
    #[test]
    fn godot_water_pipelines_foam_and_draw_validate_on_gpu() {
        futures_lite::future::block_on(async {
            let adapter = wgpu::Instance::default().request_adapter(&Default::default()).await.expect("GPU adapter");
            let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(), ..Default::default()
            }).await.expect("GPU device");
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let field = crate::WaveField::new(&device, blue_water_core::QualityTier::High);
            let waves = SurfaceWaves::new(&device, &field);
            for mode in 1..=5u8 {
                let _pipeline = OriginalSurfacePipeline::new_with_wave_layout(&device, waves.layout(),
                    wgpu::TextureFormat::Rgba16Float, 1, 16, mode, crate::shader_composer::SURFACE_GODOT_WATER);
            }
            let pipeline = OriginalSurfacePipeline::new_with_wave_layout(&device, waves.layout(),
                wgpu::TextureFormat::Rgba16Float, 1, 16, 0, crate::shader_composer::SURFACE_GODOT_WATER);
            // Patterns: a ramp in the noise channel so the foam mask passes somewhere.
            let rgba: Vec<u8> = (0..PATTERN_SIZE * PATTERN_SIZE).flat_map(|i| {
                let x = (i % PATTERN_SIZE) as u8;
                [200, x, x / 2, 255]
            }).collect();
            let patterns = GodotPatterns::new(&device, &queue, &rgba);
            let mut foam = GodotFoam::new(&device, &patterns);
            // A 4 m wide wall through the water at grid x in [-2, 2] m, everywhere else open.
            let size = [64u32, 64];
            let texels: Vec<[f32; 2]> = (0..size[0] * size[1]).map(|i| {
                let x = (i % size[0]) as f32 * 0.5 - 16.0;
                if x.abs() <= 2.0 { [3.0, -3.0] } else { [-10.0, 10.0] }
            }).collect();
            foam.set_contact(&device, &queue, Some(ContactField { texels: &texels, size, origin: [-16.0, -16.0], cell: 0.5 }));
            let mut encoder = device.create_command_encoder(&Default::default());
            let mut frame = FoamFrame { camera_xz: [0.3, 0.2], seconds: 3.0, delta: 1.0 / 60.0, scale: 1.0,
                enabled: [true, true], rate: 0.0, plane_above_field: Some(0.0) };
            foam.update(&queue, &mut encoder, &frame);
            frame.camera_xz = [1.4, 0.2];
            frame.seconds += frame.delta;
            let foam_frame = foam.update(&queue, &mut encoder, &frame);
            assert_eq!(foam_frame, [1.0, 0.0, 1.0, 0.0]);
            let mut bindings = OriginalSurfaceBindings::new(&device, &queue, &pipeline);
            let clipmap = OriginalClipmap::new(&device, 64, 6);
            let mut camera = SurfaceCamera::zeroed();
            for matrix in [&mut camera.view, &mut camera.projection, &mut camera.view_projection, &mut camera.inverse_view,
                &mut camera.inverse_projection, &mut camera.inverse_view_projection] {
                for (axis, row) in matrix.iter_mut().enumerate() { row[axis] = 1.0; }
            }
            camera.position_time = [0.0, 1.0, 0.0, 0.0];
            camera.viewport = [16.0, 16.0, 1.0 / 16.0, 1.0 / 16.0];
            camera.near = 0.025;
            camera.far = 100.0;
            camera.tan_half_fov_y = 0.5;
            camera.aspect = 1.0;
            bindings.write_camera(&queue, &camera);
            bindings.write_surface_prefix(&queue, &clipmap.surface_prefix([0.0, 0.0], 1.0, SurfaceAppearance::default()));
            bindings.write_godot_water(&queue, &[[1.0, 1.0, frame.seconds, 3.0], foam_frame, [0.0, 0.04, 0.05, 0.0], [0.0, 0.08, 0.02, 0.0]]);
            let target = |format| device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Godot water draw validation"),
                size: wgpu::Extent3d { width: 16, height: 16, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            }).create_view(&Default::default());
            let color = target(wgpu::TextureFormat::Rgba16Float);
            let depth = target(wgpu::TextureFormat::Depth32Float);
            let scene = crate::scene_copy::SceneCopy::new(&device, 16, 16, 1);
            bindings.set_scene(&device, &pipeline, &scene.color, &scene.depth);
            bindings.set_godot_water(&device, &pipeline, Some(&foam.views()));
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Godot water draw validation"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &color, depth_slice: None, resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth,
                        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
                pass.set_pipeline(&pipeline.pipeline);
                pass.set_bind_group(0, &bindings.camera_group, &[]);
                pass.set_bind_group(1, &bindings.material_group, &[]);
                pass.set_bind_group(2, waves.group(&field), &[]);
                pass.set_bind_group(3, &bindings.environment_group, &[]);
                clipmap.draw(&mut pass, 0, 0);
            }
            // Read both cascades back (Rg16Float: 1024 or 512 texels a row, 256-aligned either way).
            let readbacks: Vec<_> = foam.cascades.iter().map(|cascade| {
                let readback = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("Godot water foam readback"),
                    size: u64::from(cascade.size * cascade.size * 4),
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                encoder.copy_texture_to_buffer(cascade.output.as_image_copy(), wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(cascade.size * 4), rows_per_image: None },
                }, wgpu::Extent3d { width: cascade.size, height: cascade.size, depth_or_array_layers: 1 });
                readback
            }).collect();
            queue.submit([encoder.finish()]);
            for readback in &readbacks {
                readback.slice(..).map_async(wgpu::MapMode::Read, |result| result.expect("map foam"));
            }
            device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
            assert!(scope.pop().await.is_none(), "Godot water GPU validation");
            for (cascade, readback) in foam.cascades.iter().zip(&readbacks) {
                let bytes = readback.slice(..).get_mapped_range().expect("mapped foam").to_vec();
                let halves: &[u16] = bytemuck::cast_slice(&bytes);
                let value = |x: u32, y: u32, c: usize| half::f16::from_bits(halves[((y * cascade.size + x) * 2) as usize + c]).to_f32();
                let (mut jacobian, mut contact_wall, mut contact_open) = (0.0f32, 0.0f32, 0.0f32);
                let mut foamy = 0usize;
                for y in 0..cascade.size {
                    for x in 0..cascade.size {
                        let (j, i) = (value(x, y, 0), value(x, y, 1));
                        assert!((0.0..=1.0).contains(&j) && (0.0..=1.0).contains(&i), "foam out of range: {j} {i}");
                        jacobian = jacobian.max(j);
                        foamy += usize::from(j > 0.1);
                        // Grid x of this texel: the capture is centred at x = 1 m.
                        let grid_x = 1.0 + ((x as f32 + 0.5) / cascade.size as f32 - 0.5) * cascade.coverage;
                        if grid_x.abs() < 1.0 { contact_wall = contact_wall.max(i); }
                        if grid_x.abs() > 4.0 { contact_open = contact_open.max(i); }
                    }
                }
                let share = foamy as f32 / (cascade.size * cascade.size) as f32;
                eprintln!("cascade {} m: Jacobian foam on {:.1}% of texels", cascade.coverage, share * 100.0);
                assert!(jacobian > 0.1 && share > 0.005, "{} m: no Jacobian foam formed (max {jacobian})", cascade.coverage);
                assert!(contact_wall > 0.9, "{} m: no intersection foam inside the wall (max {contact_wall})", cascade.coverage);
                assert_eq!(contact_open, 0.0, "{} m: intersection foam in open water", cascade.coverage);
            }
        });
    }

    #[test]
    fn persistent_foam_shader_validates() {
        let source = format!("{}\n{}", include_str!("godot_water_foam.wgsl"), include_str!("godot_water.wgsl"));
        let module = wgpu::naga::front::wgsl::parse_str(&source).expect("Godot water foam parses");
        wgpu::naga::valid::Validator::new(wgpu::naga::valid::ValidationFlags::all(), wgpu::naga::valid::Capabilities::default())
            .validate(&module).expect("Godot water foam validates");
    }
}

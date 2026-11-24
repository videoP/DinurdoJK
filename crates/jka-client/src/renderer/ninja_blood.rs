//! Hybrid Overgrowth surface presentation on Ninja fluid fields.
#[path = "ninja_blood_ink.rs"]
mod ink;
use super::{Camera, Renderer, textures::GpuImage};
use crate::ninja_blood::{
    blood_pigment, BloodPool, BloodScene, BloodVertex, ENABLED_BIT, FIRST_TICK_SECONDS,
};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use jka_ninja::{BLOOD_SIM_IMAGES, BloodDesc, BloodField};
use wgpu::util::DeviceExt;

type Target = (wgpu::TextureFormat, u32, wgpu::TextureFormat);
const SEED_TICKS: u16 = 7;
#[derive(Default)]
pub(in crate::renderer) struct NinjaBloodState {
    scene: Option<BloodScene>,
    control: u64,
    failed: bool,
    gpu: Option<BloodGpu>,
}
struct BloodGpu {
    target: Target,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    images: [GpuImage; BLOOD_SIM_IMAGES],
    atlas: Option<ink::InkMasks>,
    /// Full-size ink raster, kept all-zero between pools (see `ink::take`).
    scratch: Vec<u8>,
    /// Tightly packed dirty rectangle handed to the Ninja ink upload.
    upload: Vec<u8>,
    desc: BloodDesc,
    slots: Vec<BloodDraw>,
}
struct BloodDraw {
    id: Option<u64>,
    field: BloodField,
    uniform: wgpu::Buffer,
    group: Option<wgpu::BindGroup>,
    mesh: (wgpu::Buffer, u32),
    last_ms: Option<i32>,
    motion_ms: Option<i32>,
    coast_ticks: u16,
    seeded: bool,
    trail_count: usize,
    stamp_count: usize,
    smear_count: usize,
    /// Rect last written to this field's ink image; the next upload covers
    /// it too so stale ink is zeroed (slot reuse keeps the same texture).
    ink_rect: Option<ink::InkRect>,
    distance2: f32,
}
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BloodUniform {
    view_proj: [[f32; 4]; 4],
    tangent: [f32; 4],
    bitangent: [f32; 4],
    eye: [f32; 4],
    ambient: [f32; 4],
    directed: [f32; 4],
    direction: [f32; 4],
    pigment: [f32; 4],
}
impl NinjaBloodState {
    fn set_scene(&mut self, scene: Option<BloodScene>) {
        let control = crate::ninja_blood::token();
        if self.control != control || scene.is_none() {
            self.gpu = None;
            self.failed = false;
        }
        self.control = control;
        self.scene = scene.filter(|s| s.control == control && control & ENABLED_BIT != 0);
        if self.scene.is_none() {
            self.gpu = None;
        }
    }
    pub(in crate::renderer) fn wants_frame(&self) -> bool {
        !self.failed
            && self.control & ENABLED_BIT != 0
            && self.control == crate::ninja_blood::token()
            && self.scene.as_ref().is_some_and(|s| !s.pools.is_empty())
    }
    pub(in crate::renderer) fn draw(&self, pass: &mut wgpu::RenderPass<'_>) -> bool {
        if !self.wants_frame() {
            return false;
        }
        let Some(gpu) = self.gpu.as_ref() else {
            return false;
        };
        let mut drawn = false;
        for slot in gpu.slots.iter().filter(|s| s.id.is_some()) {
            let Some(group) = slot.group.as_ref() else {
                continue;
            };
            if !drawn {
                pass.set_pipeline(&gpu.pipeline);
                drawn = true;
            }
            pass.set_bind_group(0, group, &[]);
            pass.set_vertex_buffer(0, slot.mesh.0.slice(..));
            pass.draw(0..slot.mesh.1, 0..1);
        }
        drawn
    }
}
impl Renderer {
    pub(in crate::renderer) fn set_ninja_blood_scene(&mut self, scene: Option<BloodScene>) {
        self.ninja_blood.set_scene(scene);
    }
    pub(in crate::renderer) fn encode_ninja_blood(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        camera: &Camera,
        view_proj: Mat4,
        format: wgpu::TextureFormat,
    ) {
        if !self.ninja_blood.wants_frame() {
            return;
        }
        let Some(scene) = self.ninja_blood.scene.clone() else {
            return;
        };
        let target = (format, self.msaa_samples, super::depth_format());
        let result = (|| -> Result<(), String> {
            if self
                .ninja_blood
                .gpu
                .as_ref()
                .is_none_or(|gpu| gpu.target != target)
            {
                let mut gpu = BloodGpu::new(self, target)?;
                if let Some(previous) = self.ninja_blood.gpu.take() {
                    gpu.slots = previous.slots;
                    for slot in &mut gpu.slots {
                        slot.group = None;
                    }
                }
                self.ninja_blood.gpu = Some(gpu);
            }
            let gpu = self
                .ninja_blood
                .gpu
                .as_mut()
                .expect("blood GPU initialized");
            let light_grid = self
                .world
                .as_ref()
                .and_then(|w| w.entity_light_grid.as_ref());
            // Retire replaced pools before taking reusable slots.
            for slot in &mut gpu.slots {
                if slot
                    .id
                    .is_some_and(|id| !scene.pools.iter().any(|pool| pool.id == id))
                {
                    slot.id = None;
                }
            }
            for pool in scene.pools.iter() {
                let slot = if let Some(i) = gpu.slots.iter().position(|s| s.id == Some(pool.id)) {
                    &mut gpu.slots[i]
                } else if let Some(i) = gpu.slots.iter().position(|s| s.id.is_none()) {
                    let slot = &mut gpu.slots[i];
                    if slot.field.high_quality() != pool.seed.high_quality {
                        // A reused pool slot can switch between a floor and a
                        // vertical receiver. Ninja's fixed-size grid cannot be
                        // reset at a different resolution; recreate only then.
                        slot.field = BloodField::new(&self.device,
                            gpu.images.each_ref().map(|image| &image.view), pool.seed)?;
                        slot.ink_rect = None;
                    } else {
                        slot.field.restart(&self.device, encoder, pool.seed)?;
                    }
                    slot.id = Some(pool.id);
                    slot.mesh = BloodDraw::mesh(&self.device, pool);
                    slot.last_ms = None;
                    slot.motion_ms = None;
                    slot.coast_ticks = SEED_TICKS;
                    slot.seeded = false;
                    slot.trail_count = 0;
                    slot.stamp_count = 0;
                    slot.smear_count = 0;
                    slot.group = None;
                    slot
                } else {
                    gpu.slots.push(BloodDraw::new(
                        &self.device,
                        &gpu.images,
                        pool,
                    )?);
                    gpu.slots.last_mut().expect("blood slot added")
                };
                let previous = slot.last_ms.replace(scene.time_ms);
                let motion = pool.motion.as_ref()
                    .filter(|(time, _)| previous.is_some() && slot.motion_ms != Some(*time));
                let new_trails = pool.trail.len() > slot.trail_count;
                let new_seed = !slot.seeded;
                let atlas_seed = new_seed && pool.seeded_impact && gpu.atlas.is_some();
                let new_stamps = pool.stamps.len() > slot.stamp_count
                    || pool.smears.len() > slot.smear_count;
                let need_ink = atlas_seed || new_trails || new_stamps;
                // Do not treat a fixed number of solver ticks as physical
                // drying. Older stains may still be moved by new foot contact;
                // the simulation coasts only when disturbed, then idles.
                if (new_seed || pool.fade_started_ms.is_none())
                    && (new_seed || slot.coast_ticks > 0 || motion.is_some() || new_trails || new_stamps)
                {
                    let delta = previous.map_or(FIRST_TICK_SECONDS, |last| {
                        f64::from(scene.time_ms.saturating_sub(last).clamp(0, 33)) / 1000.0
                    });
                    // Upload only the new ink's rect plus the rect inked last
                    // time (now zero), instead of clearing/uploading 512².
                    let mut drawn = None;
                    let upload = if need_ink {
                        drawn = ink::fill(&mut gpu.scratch, pool, gpu.atlas.as_ref(), atlas_seed,
                            slot.trail_count, slot.stamp_count, slot.smear_count);
                        let rect = ink::union(drawn, slot.ink_rect);
                        if let Some(rect) = rect { ink::take(&mut gpu.scratch, rect, &mut gpu.upload); }
                        rect
                    } else { None };
                    let ink_input = upload.map(|[x0, y0, x1, y1]| jka_ninja::BloodInk {
                        bytes: &gpu.upload, origin: [x0, y0], size: [x1 - x0, y1 - y0],
                    });
                    if slot.field.encode(
                        &self.queue, encoder, delta,
                        motion.map_or(&[][..], |(_, motions)| motions.as_slice()),
                        ink_input,
                        atlas_seed || !pool.seeded_impact,
                    )? {
                        if upload.is_some() { slot.ink_rect = drawn; }
                        slot.seeded = true;
                        slot.trail_count = pool.trail.len();
                        slot.stamp_count = pool.stamps.len();
                        slot.smear_count = pool.smears.len();
                        if let Some((time, _)) = motion.filter(|_| !new_seed) {
                            slot.motion_ms = Some(*time);
                            slot.coast_ticks = gpu.desc.coast_ticks;
                        }
                        if new_trails || new_stamps { slot.coast_ticks = slot.coast_ticks.max(SEED_TICKS); }
                        slot.coast_ticks = slot.coast_ticks.saturating_sub(1);
                    }
                }
                let outputs = slot.field.outputs();
                let Some(vd) = outputs.velocity_density else {
                    continue;
                };
                if slot.group.is_none() {
                    let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
                    let memory = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
                    let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("Ninja blood output bindings"),
                        layout: &gpu.layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: slot.uniform.as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(vd),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::Sampler(&gpu.sampler),
                            },
                        ],
                    });
                    resolve_scopes(memory, validation)?;
                    slot.group = Some(group);
                }
                let render_vector =
                    |v: [f32; 3]| Vec3::from_array(crate::scene::render_position(v));
                let centre = render_vector(pool.origin);
                let lighting = light_grid
                    .and_then(|g| g.sample_classic_entity_light(pool.origin))
                    .unwrap_or_default();
                let wetness = 1.0 - (scene.time_ms.saturating_sub(pool.last_ink_ms).max(0) as f32
                    / 10_000.0).clamp(0.0, 1.0);
                let u = BloodUniform {
                    view_proj: view_proj.to_cols_array_2d(),
                    tangent: render_vector(pool.tangent.to_array())
                        // Overgrowth world decals dry linearly for ten seconds.
                        .extend(wetness)
                        .to_array(),
                    bitangent: render_vector(pool.bitangent.to_array())
                        .extend(0.0)
                        .to_array(),
                    eye: camera.position.extend(0.0).to_array(),
                    ambient: (Vec3::from_array(lighting.ambient) / 255.0)
                        .extend(0.0)
                        .to_array(),
                    directed: (Vec3::from_array(lighting.directed) / 255.0)
                        .extend(0.0)
                        .to_array(),
                    direction: Vec3::from_array(lighting.direction).extend(0.0).to_array(),
                    pigment: Vec3::from_array(blood_pigment(wetness)).extend(pool.opacity(scene.time_ms)).to_array(),
                };
                slot.distance2 = camera.position.distance_squared(centre);
                self.queue
                    .write_buffer(&slot.uniform, 0, bytemuck::bytes_of(&u));
            }
            gpu.slots
                .sort_unstable_by(|a, b| b.distance2.total_cmp(&a.distance2));
            Ok(())
        })();
        if let Err(error) = result {
            log::warn!("[NINJA BLOOD] disabled until reset ({error})");
            self.ninja_blood.gpu = None;
            self.ninja_blood.failed = true;
        }
    }
}

fn resolve_scopes(
    memory: wgpu::ErrorScopeGuard,
    validation: wgpu::ErrorScopeGuard,
) -> Result<(), String> {
    let m = jka_river::river_ninja::checked_scope(memory);
    let v = jka_river::river_ninja::checked_scope(validation);
    v.and(m)
}
impl BloodGpu {
    fn new(renderer: &Renderer, target: Target) -> Result<Self, String> {
        let desc = jka_ninja::blood_desc().ok_or("private blood solver not included")?;
        // No material fallback. Check before loading textures/allocating.
        let material =
            jka_ninja::blood_material_wgsl().ok_or("UE blood material helpers are not included")?;
        let specs = jka_ninja::blood_input_images().ok_or("private blood images not included")?;
        let mut search = super::AssetSearchPath::open_game(
            &renderer.base,
            renderer.detail_texture_game.as_deref(),
        )
        .map_err(|e| e.to_string())?;
        let mut decoded = Vec::with_capacity(BLOOD_SIM_IMAGES);
        for spec in specs {
            let asset = search
                .read(spec.qpath, 16 * 1024 * 1024)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("required local texture missing: {}", spec.qpath))?;
            let image = crate::materials::decode_texture_data_with_color_space(
                spec.qpath,
                &asset.bytes,
                false,
                spec.mipmaps,
                spec.srgb,
            )?;
            if [image.width, image.height] != spec.size {
                return Err(format!(
                    "{} is {}x{}, expected {}x{}",
                    spec.qpath, image.width, image.height, spec.size[0], spec.size[1]
                ));
            }
            decoded.push(image);
        }
        let atlas = ink::InkMasks::load(&mut search);
        let device = &renderer.device;
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let images = std::array::from_fn(|i| {
            super::textures::upload_texture(device, &renderer.queue, &decoded[i])
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Ninja blood render target clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty,
            count: None,
        };
        let sample = || wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ninja Blood1"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<BloodUniform>() as u64,
                        ),
                    },
                    count: None,
                },
                entry(1, sample()),
                entry(
                    2,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Ninja Blood1"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ninja Blood1"),
            source: wgpu::ShaderSource::Wgsl(
                format!("{}\n{material}", include_str!("ninja_blood.wgsl")).into(),
            ),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Ninja Blood1 translucent surface"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("blood_vertex"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<BloodVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
                })],
                compilation_options: Default::default(),
            },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: target.2,
                depth_write_enabled: Some(false),
                depth_compare: Some(super::camera_depth_compare()),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: target.1,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("blood_fragment"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target.0,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        resolve_scopes(memory, validation)?;
        Ok(Self {
            target,
            layout,
            pipeline,
            sampler,
            images,
            atlas,
            scratch: Vec::new(),
            upload: Vec::new(),
            desc,
            slots: Vec::new(),
        })
    }
}
impl BloodDraw {
    fn new(
        device: &wgpu::Device,
        images: &[GpuImage; BLOOD_SIM_IMAGES],
        pool: &BloodPool,
    ) -> Result<Self, String> {
        let field = BloodField::new(
            device,
            images.each_ref().map(|image| &image.view),
            pool.seed,
        )?;
        let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ninja blood placement and light"),
            size: std::mem::size_of::<BloodUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        resolve_scopes(memory, validation)?;
        Ok(Self {
            id: Some(pool.id),
            field,
            uniform,
            group: None,
            mesh: Self::mesh(device, pool),
            last_ms: None,
            motion_ms: None,
            coast_ticks: SEED_TICKS,
            seeded: false,
            trail_count: 0,
            stamp_count: 0,
            smear_count: 0,
            ink_rect: None,
            distance2: 0.0,
        })
    }

    fn mesh(device: &wgpu::Device, pool: &BloodPool) -> (wgpu::Buffer, u32) {
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Projected blood surface"),
            contents: bytemuck::cast_slice(&pool.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        (buffer, pool.vertices.len() as u32)
    }
}

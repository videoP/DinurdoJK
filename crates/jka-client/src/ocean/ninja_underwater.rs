//! Underwater mode 5 (r_blueWaterUnderwaterMode / r_hybridUnderwaterMode /
//! r_lakeUnderwaterMode): the Fluid Ninja river's submerged post material
//! (Green Murky v2, `jka_ninja::underwater_wgsl`) in place of Blue Water's
//! dim/light composite and motes, or the lakes' Godot submerged fog.
//! One fullscreen pass over the post-water scene copy; the Blue Water probe
//! or the flat-volume water path masks dry pixels. Built on first submerged
//! use; one pipeline per target.

/// `NinjaOceanView` in ninja_underwater.wgsl (the river's `RiverView` subset).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct View {
    pub eye: [f32; 4],
    pub forward: [f32; 4],
    pub right: [f32; 4],
    pub up: [f32; 4],
    pub distortion: [f32; 4],
    pub sun_direction: [f32; 4],
    pub mask: [f32; 4],
}

/// Which per-pixel wet test the pass uses.
pub(crate) enum Mask<'a> {
    /// Blue Water's underwater probe (metres below the waves, filtered).
    Probe(&'a wgpu::TextureView),
    /// `optics.water_distance`: the flat JKA volume path length (R32Float).
    Path(&'a wgpu::TextureView),
}

impl View {
    /// The river host's view, with its Ninja time and mode lanes set.
    /// `distance_mult` is the fog density (`OceanSettings::ninja_underwater_distance_mult`).
    pub(crate) fn from_river(view: &crate::river_surface::ViewUniform, time: f32, distance_mult: f32) -> Self {
        Self {
            eye: view.eye,
            forward: view.forward,
            right: [view.right[0], view.right[1], view.right[2], time],
            up: [view.up[0], view.up[1], view.up[2], 1.0],
            distortion: view.distortion,
            sun_direction: view.sun_direction,
            mask: { let [start, end] = fade_range_cm(distance_mult); [0.0, start, end, distance_mult] },
        }
    }
}

/// Green Murky DistanceMult scaled by the water's Transparency so every fog
/// distance (full DistanceColor, host fade end, cull) scales linearly with it.
pub(crate) fn distance_mult(transparency: f32) -> f32 {
    jka_ninja::NinjaUnderwaterSettings::default().distance_mult
        * transparency.clamp(0.1, 10.0).powf(-DISTANCE_EXPONENT)
}

/// M_UnderWaterPostProcess `pow(depth_cm, DistanceExponent) * DistanceMult + DistanceOffset`
/// constants as the private port bakes them (river_underwater.wgsl).
const DISTANCE_EXPONENT: f32 = 0.305;
const DISTANCE_OFFSET: f32 = -0.8;
/// The host fade runs from full DistanceColor to this multiple of that depth.
const FADE_END_FACTOR: f32 = 2.0;
const CM_PER_UNIT: f32 = 2.5;

/// Radial cm where the scene fades out: from the depth where DistanceColor
/// is fully reached (`saturate` = 1) to `FADE_END_FACTOR` times it.
fn fade_range_cm(distance_mult: f32) -> [f32; 2] {
    let mult = distance_mult.max(1e-4);
    let full = ((1.0 - DISTANCE_OFFSET) / mult).powf(1.0 / DISTANCE_EXPONENT);
    [full, full * FADE_END_FACTOR]
}

/// Underwater mode 5 world/clipmap cull distance (JKA units): nothing past
/// the end of the host fade is visible.
pub(crate) fn cull_distance_units(distance_mult: f32) -> f32 {
    fade_range_cm(distance_mult)[1] / CM_PER_UNIT
}

/// M_UnderWaterPostProcess noise and gradient, and the river caustics volume.
pub(crate) struct Textures {
    pub noise: wgpu::TextureView,
    pub gradient: wgpu::TextureView,
    pub caustics: Option<wgpu::TextureView>,
}

pub(super) struct NinjaUnderwater {
    module: wgpu::ShaderModule,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    pipeline: Option<((wgpu::TextureFormat, u32), wgpu::RenderPipeline)>,
    view: wgpu::Buffer,
    authored: wgpu::Buffer,
    linear: wgpu::Sampler,
    wrap: wgpu::Sampler,
    inert_volume: wgpu::TextureView,
    inert_probe: wgpu::TextureView,
    inert_path: wgpu::TextureView,
}

impl NinjaUnderwater {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let source = format!("{}\n{}", include_str!("ninja_underwater.wgsl"), jka_ninja::underwater_wgsl());
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ninja ocean underwater"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let fragment = wgpu::ShaderStages::FRAGMENT;
        let uniform = |binding| wgpu::BindGroupLayoutEntry { binding, visibility: fragment, ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None };
        let texture = |binding, sample_type, view_dimension| wgpu::BindGroupLayoutEntry { binding, visibility: fragment,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension, multisampled: false }, count: None };
        let sampler = |binding| wgpu::BindGroupLayoutEntry { binding, visibility: fragment,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None };
        let float = wgpu::TextureSampleType::Float { filterable: true };
        let d2 = wgpu::TextureViewDimension::D2;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ninja ocean underwater"),
            entries: &[uniform(0), uniform(1), texture(2, float, d2), texture(3, wgpu::TextureSampleType::Depth, d2),
                texture(4, float, d2), sampler(5), sampler(6), texture(7, float, d2), texture(8, float, d2),
                texture(9, float, wgpu::TextureViewDimension::D3),
                texture(10, wgpu::TextureSampleType::Float { filterable: false }, d2)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Ninja ocean underwater"), bind_group_layouts: &[Some(&layout)], immediate_size: 0,
        });
        let buffer = |label, size| device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label), size, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Ninja ocean underwater clamp"), mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let wrap = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Ninja ocean underwater wrap"), address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat, address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear, ..Default::default() });
        let inert = |label, dimension, format| device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
            dimension, format, usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
        }).create_view(&Default::default());
        let inert_volume = inert("Ninja ocean underwater inert caustics", wgpu::TextureDimension::D3, wgpu::TextureFormat::Rgba8Unorm);
        let inert_probe = inert("Ninja ocean underwater inert probe", wgpu::TextureDimension::D2, wgpu::TextureFormat::Rgba16Float);
        let inert_path = inert("Ninja ocean underwater inert path", wgpu::TextureDimension::D2, wgpu::TextureFormat::R32Float);
        Self { module, layout, pipeline_layout, pipeline: None,
            view: buffer("Ninja ocean underwater view", std::mem::size_of::<View>() as u64),
            authored: buffer("Ninja ocean underwater controls", 48), linear, wrap, inert_volume, inert_probe, inert_path }
    }

    /// Writes this frame's controls and returns the bind group.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, samples: u32,
        textures: &Textures, mut view: View, scene: &blue_water_gpu::scene_copy::SceneCopy,
        mask: Mask<'_>) -> wgpu::BindGroup {
        let (probe, path) = match mask {
            Mask::Probe(probe) => { view.mask[0] = 0.0; (probe, &self.inert_path) }
            Mask::Path(path) => { view.mask[0] = 1.0; (&self.inert_probe, path) }
        };
        let (noise, gradient) = (&textures.noise, &textures.gradient);
        if self.pipeline.as_ref().is_none_or(|(key, _)| *key != (format, samples)) {
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Ninja ocean underwater"), layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState { module: &self.module, entry_point: Some("vs_ninja_ocean_underwater"),
                    compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState { module: &self.module, entry_point: Some("fs_ninja_ocean_underwater"),
                    compilation_options: Default::default(), targets: &[Some(format.into())] }),
                // The shared underwater pass keeps the scene depth attached; read-only, always passes.
                primitive: Default::default(), depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Always), stencil: Default::default(), bias: Default::default() }),
                multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
                multiview_mask: None, cache: None,
            });
            self.pipeline = Some(((format, samples), pipeline));
        }
        let caustics = textures.caustics.as_ref();
        view.up[3] = if caustics.is_some() { 2.0 } else { 1.0 };
        let ninja = jka_ninja::NinjaUnderwaterSettings::default();
        let authored: [[f32; 4]; 3] = [
            [view.mask[3], ninja.distortion, ninja.brightness, ninja.caustics_mult],
            [ninja.distance_color[0], ninja.distance_color[1], ninja.distance_color[2], 0.0],
            [ninja.caustics_color[0], ninja.caustics_color[1], ninja.caustics_color[2], 0.0],
        ];
        queue.write_buffer(&self.view, 0, bytemuck::bytes_of(&view));
        queue.write_buffer(&self.authored, 0, bytemuck::cast_slice(&authored));
        let t = |binding, view| wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(view) };
        let s = |binding, sampler| wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::Sampler(sampler) };
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Ninja ocean underwater"), layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.view.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.authored.as_entire_binding() },
                t(2, &scene.color), t(3, &scene.depth), t(4, probe), s(5, &self.linear), s(6, &self.wrap),
                t(7, noise), t(8, gradient), t(9, caustics.unwrap_or(&self.inert_volume)), t(10, path),
            ],
        })
    }

    pub(super) fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, group: &'a wgpu::BindGroup) {
        let Some((_, pipeline)) = self.pipeline.as_ref() else { return };
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }
}

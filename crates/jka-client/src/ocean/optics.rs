//! Screen-space water optics. Capture precedes ocean and debug draws; never
//! sample the live color/depth attachments. See Development Docs/Ocean optics audit.md.
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

pub fn linear_color(rgb: [f32; 3]) -> [f32; 3] {
    rgb.map(|c| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) })
}

mod cull;
mod procedural;
pub use cull::{submerged_cull_radius, WaterCull};

/// Absorption distances of water before culling may remove anything. Blue light
/// is 5% transmitted after 1 absorption distance, 0.2% after 2, 0.007% after 3.
pub const MIN_CULL_MULTIPLE: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    pub fog_color: [f32; 3],
    pub transparency: f32,
    pub depth_darkening: f32,
    pub refraction: f32,
    pub caustics: f32,
    /// Maximum camera-to-receiver distance for Godot and Hybrid Godot caustics,
    /// in metres. The final 25% fades smoothly; 0 preserves unbounded legacy
    /// behavior. Native Blue Water caustics keep their own visibility policy.
    pub caustic_max_distance_m: f32,
    /// Water-hidden geometry cull, as a multiple of the absorption distance: the
    /// distance past which a submerged view sees nothing, and the water depth that
    /// hides a batch from above. Zero disables. See `cull.rs`.
    pub underwater_cull: f32,
    /// Skip the whole capture when the camera is dry and no ocean clipmap
    /// survived this frame's culling. The capture is read only by the ocean
    /// surface shader (bsp.wgsl) and the submerged composite, so with neither
    /// present its result cannot be observed.
    pub cull: bool,
    /// Restrict the capture to the screen rectangle the visible water covers
    /// instead of the whole framebuffer. The surface shader samples the capture
    /// at its own fragment plus a refraction offset bounded by `refraction`, so
    /// a padded water rect contains every read.
    pub scissor: bool,
    /// Honour each water volume's outline mask in the capture. Off makes every
    /// volume a plain box, which is visually wrong on hollow water (an island
    /// ring's centre counts as submerged) but isolates the cost of
    /// `add_masked_spans`: that runs per pixel, per volume hit, looping every
    /// mask triangle while maintaining a dynamically indexed local array.
    /// Purely a measurement lever - no shader change, the mask slot simply
    /// stops being written into the uniform.
    pub masks: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self { fog_color: [20.0 / 255.0, 40.0 / 255.0, 50.0 / 255.0], transparency: 2304.0,
            depth_darkening: 1.0, refraction: 0.1, caustics: 1.0, caustic_max_distance_m: 80.0, underwater_cull: 3.0,
            // The scissor is off by default: measured on a real map, a promoted
            // surface's bounds surround the camera, so there is no tight screen
            // rectangle to find and it falls back to full-screen anyway. Kept
            // only as an A/B lever.
            cull: true, scissor: false, masks: true }
    }
}
impl Settings {
    pub fn sanitize(mut self) -> Self {
        fn valid(x: f32, fallback: f32, lo: f32, hi: f32) -> f32 {
            if x.is_finite() { x.clamp(lo, hi) } else { fallback }
        }
        let d = Self::default();
        for i in 0..3 { self.fog_color[i] = valid(self.fog_color[i], d.fog_color[i], 0.0, 1.0); }
        self.transparency = valid(self.transparency, d.transparency, 1.0, 65536.0);
        self.depth_darkening = valid(self.depth_darkening, d.depth_darkening, 0.01, 32.0);
        self.refraction = valid(self.refraction, d.refraction, 0.0, 0.15);
        self.caustics = valid(self.caustics, d.caustics, 0.0, 8.0);
        self.caustic_max_distance_m = valid(self.caustic_max_distance_m, d.caustic_max_distance_m, 0.0, 500.0);
        self.underwater_cull = valid(self.underwater_cull, d.underwater_cull, 0.0, 32.0);
        self
    }
    pub fn absorption_distance(self) -> f32 { self.transparency }
    /// Distance (in water) past which geometry is culled, or 0 when disabled. The
    /// multiple is floored at `MIN_CULL_MULTIPLE`: below it the cull would remove
    /// geometry that is still plainly visible through the water.
    pub fn cull_distance(self) -> f32 {
        if self.underwater_cull <= 0.0 { 0.0 }
        else { self.absorption_distance() * self.underwater_cull.max(MIN_CULL_MULTIPLE) }
    }
}

/// Render-only ocean continuation. No finite XZ bound or added coastal volume.
/// Every segment represents one sky-contact half-strip; the four corner bits
/// admit quadrants only when both adjoining contacted sky edges meet there.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
pub struct SkyFogRegion {
    pub bounds: [f32; 4],
    pub plane: [f32; 4], // top Y, segment count, corner bitmask, bottom Y
    pub segments: [[f32; 4]; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Uniform {
    pub inverse_view_projection: [[f32; 4]; 4],
    pub eye: [f32; 4],
    pub forward: [f32; 4], // xyz view direction, w fixed floor below the submerged eye
    pub fog: [f32; 4], // linear color, absorption distance
    pub params: [f32; 4], // darkening distance, refraction, caustics, volume count
    pub minimum: [[f32; 4]; 8],
    pub maximum: [[f32; 4]; 8],
    // Only populated for r_oceanSkyExtend and qualifying map sky contacts.
    pub sky_regions: [SkyFogRegion; 8],
    pub sky_info: [f32; 4], // active continuation count; zero for OFF
    /// Caustics: spatial density, focus gain, optional 3-tap derivative,
    /// camera-to-receiver distance limit in JKA units (0 = unlimited).
    pub caustic_tuning: [f32; 4],
    /// Procedural caustics: time (seconds), density, wave-responsive speed, chop response (0 = disabled).
    pub procedural_params: [f32; 4],
    /// Hybrid wave optics: x = correction added to the shader's two-cascade wave
    /// height near the eye (JKA units), y = 1 when the CPU probe is valid.
    pub eye_wave: [f32; 4],
}

impl Uniform {
    /// Find this surface's floor in the shared authored/promoted volume list.
    pub fn surface_bottom(&self, surface: super::OceanSurface) -> f32 {
        (0..(self.params[3] as usize).min(8)).filter(|&i| {
            (self.maximum[i][1] - surface.plane_height).abs() <= 1.0
                && self.minimum[i][0] <= surface.minimum[0] + 0.75
                && self.minimum[i][2] <= surface.minimum[1] + 0.75
                && self.maximum[i][0] >= surface.maximum[0] - 0.75
                && self.maximum[i][2] >= surface.maximum[1] - 0.75
        }).map(|i| self.minimum[i][1]).reduce(f32::min).unwrap_or(surface.plane_height)
    }
}

pub fn layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    make_layout(device, false, false)
}
fn make_layout(device: &wgpu::Device, capture: bool, msaa: bool) -> wgpu::BindGroupLayout {
    let entry = |binding, ty| wgpu::BindGroupLayoutEntry {
        binding, visibility: wgpu::ShaderStages::FRAGMENT, ty, count: None,
    };
    let mut entries = vec![
        entry(0, wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: !(capture && msaa) },
            view_dimension: wgpu::TextureViewDimension::D2, multisampled: capture && msaa,
        }),
        entry(1, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)),
        entry(2, wgpu::BindingType::Texture { sample_type: if capture { wgpu::TextureSampleType::Depth } else { wgpu::TextureSampleType::Float { filterable: false } }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: capture && msaa }),
        entry(3, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }),
    ];
    if capture {
        entries.push(entry(4, wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false,
        }));
        entries.push(entry(5, wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)));
    } else {
        entries.push(entry(4, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }));
        for binding in [5, 6, 7] {
            entries.push(entry(binding, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }));
        }
    }
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Ocean optics capture/sampling layout"),
        entries: &entries,
    })
}

pub struct Resources {
    pub key: (u32, u32, u32, wgpu::TextureFormat),
    pub bind_group: wgpu::BindGroup,
    pub uniform: wgpu::Buffer,
    pub glass_uniform: wgpu::Buffer,
    pub edges: crate::glass::edges::Resources,
    capture_color: wgpu::TextureView,
    pub receiver_color: wgpu::TextureView,
    background: wgpu::TextureView,
    background_ready: bool,
    color: wgpu::TextureView,
    background_capture: wgpu::RenderPipeline,
    blur_first: wgpu::RenderPipeline,
    blur_next: wgpu::RenderPipeline,
    blur_groups: Vec<wgpu::BindGroup>,
    blur_layout: wgpu::BindGroupLayout,
    blur_inputs: Vec<wgpu::TextureView>,
    blur_outputs: Vec<wgpu::TextureView>,
    capture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    depth: wgpu::TextureView,
    pub water_distance: wgpu::TextureView,
    procedural: procedural::Pattern,
    water_distance_capture: wgpu::RenderPipeline,
    distance_params: wgpu::Buffer,
    distance_group: wgpu::BindGroup,
    capture: wgpu::RenderPipeline,
    capture_receiver: wgpu::RenderPipeline,
    // Lazily specialized: non-Hybrid shaders compile the wave path OUT.
    hybrid_capture: Option<(wgpu::RenderPipeline, wgpu::RenderPipeline)>,
    // Lazily specialized: submerged Hybrid mode 4 fogs by ray length alone.
    submerged_capture: Option<(wgpu::RenderPipeline, wgpu::RenderPipeline)>,
    // Lazily built: the receiver output alone (`receiver_only_source`), for frames where nothing reads the rest.
    receiver_only: Option<wgpu::RenderPipeline>,
    blit: wgpu::RenderPipeline,
}
impl Resources {
    /// Hybrid fog capture, reusable by the animated lake underside without
    /// another scene copy. Depth is linear in host world units; sky is 1e6.
    pub fn submerged_scene(&self) -> (&wgpu::TextureView, &wgpu::TextureView) {
        (&self.capture_color, &self.depth)
    }

    pub fn new(device: &wgpu::Device, output_layout: &wgpu::BindGroupLayout,
        ocean_layout: &wgpu::BindGroupLayout, key: (u32, u32, u32, wgpu::TextureFormat)) -> Self {
        let (width, height, samples, format) = key;
        let target = |format, label| device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label), size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
        }).create_view(&Default::default());
        // Keep premultiplied blur validity precise near excluded foregrounds.
        let optical_format = wgpu::TextureFormat::Rgba16Float;
        let mip_count = (width.max(height).ilog2() + 1).min(7);
        let color_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Shared optical scene / Gaussian blur pyramid"), size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: mip_count, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: optical_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
        });
        let color = color_texture.create_view(&Default::default());
        let mip_view = |mip| color_texture.create_view(&wgpu::TextureViewDescriptor { base_mip_level: mip, mip_level_count: Some(1), ..Default::default() });
        let capture_color = mip_view(0);
        let receiver_color = target(optical_format, "Godot caustics before BlueWater absorption");
        let depth = target(wgpu::TextureFormat::R32Float, "Ocean pre-surface linear depth");
        let water_distance = target(wgpu::TextureFormat::R32Float, "Blue Water bounded ray length in metres");
        let background = background_target(device, 1, 1);
        let edges = crate::glass::edges::Resources::new(device, width, height, samples, &depth);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Ocean clamp sampler"), mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear, mipmap_filter: wgpu::MipmapFilterMode::Linear, ..Default::default()
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ocean optical volume uniform"), contents: bytemuck::bytes_of(&Uniform::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let glass_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Live glass settings"),
            contents: bytemuck::bytes_of(&crate::glass::settings().uniform([0.1; 3])),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = bind(device, output_layout, &color, &depth, &sampler, &uniform, Some((&glass_uniform, &edges.field, &edges.mask, &background)), None);
        let capture_layout = make_layout(device, true, samples > 1);
        let capture_source = shader_source(samples > 1);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ocean optical capture"), source: wgpu::ShaderSource::Wgsl(capture_source.into()),
        });
        let capture = pipeline(device, &shader, &[&capture_layout, ocean_layout], "fs_capture", &[optical_format, wgpu::TextureFormat::R32Float], 1);
        let capture_receiver = pipeline(device, &shader, &[&capture_layout, ocean_layout], "fs_capture_receiver", &[optical_format, wgpu::TextureFormat::R32Float, optical_format], 1);
        // The original underwater probe admits the near-plane point first.
        // Its open-water rays must continue through qualified sky contacts even
        // without a finite receiver; the legacy color capture keeps its rule.
        let source = shader_source(samples > 1).replace("\r\n", "\n");
        let sky_guard = "if (depth > 0.0) {\n        add_sky_ocean_spans(delta);\n    }";
        assert_eq!(source.matches(sky_guard).count(), 1);
        let source = source.replace(sky_guard, "add_sky_ocean_spans(delta);")
            .replace("let world = h.xyz / h.w;", "let receiver = h.xyz / h.w;\n    let world = select(receiver, optics.eye.xyz + normalize(receiver - optics.eye.xyz) * 400000.0, depth <= 0.0);")
            .replace("run.y >= merged.x - 1.0e-4 && run.x <= merged.y + 1.0e-4",
                "run.y >= merged.x - 1.0e-7 && run.x <= merged.y + 1.0e-7")
            .replace("let hi = optics.maximum[i].xyz;", "let top = optics.maximum[i].xyz;\n        let hi = vec3<f32>(top.x, jka_wave_top(top.y), top.z);")
            .replace("region.plane.x", "jka_wave_top(region.plane.x)")
            .replace("optics.sky_regions[i].plane.x", "jka_wave_top(optics.sky_regions[i].plane.x)");
        let source = format!("{source}\n@group(2) @binding(0) var<uniform> jka_wave_envelope: vec4<f32>;\nfn jka_wave_top(top: f32) -> f32 {{ return top + select(0.0, jka_wave_envelope.y, abs(top - jka_wave_envelope.x) < 0.75); }}");
        let distance_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Blue Water wave envelope"), entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false,
                    min_binding_size: None }, count: None }],
        });
        let distance_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Blue Water wave envelope"), size: 16, mapped_at_creation: false,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let distance_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blue Water wave envelope"), layout: &distance_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: distance_params.as_entire_binding() }],
        });
        let distance_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Blue Water bounded underwater path"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let water_distance_capture = pipeline(device, &distance_shader, &[&capture_layout, ocean_layout, &distance_layout],
            "fs_water_distance", &[wgpu::TextureFormat::R32Float], 1);
        let background_capture = pipeline(device, &shader, &[&capture_layout, ocean_layout], "fs_glass_background", &[optical_format], 1);
        let blit_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ocean optical composite"), source: wgpu::ShaderSource::Wgsl(include_str!("../ocean_composite.wgsl").into()),
        });
        let blit = pipeline(device, &blit_shader, &[output_layout], "fs_main", &[format], samples);
        let texture_entry = |binding, filterable| wgpu::BindGroupLayoutEntry {
            binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false,
            }, count: None,
        };
        let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("Glass Gaussian pyramid inputs"), entries: &[
            texture_entry(0, true), wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            texture_entry(2, false), texture_entry(3, false), texture_entry(4, false),
            wgpu::BindGroupLayoutEntry { binding: 5, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            // Glass bounds: each level draws only the rectangle glass can sample.
            wgpu::BindGroupLayoutEntry { binding: 6, visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
        ] });
        let blur_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("Foreground-safe glass blur"), source: wgpu::ShaderSource::Wgsl(include_str!("../glass_blur.wgsl").into()) });
        let blur_first = pipeline(device, &blur_shader, &[&blur_layout], "fs_first", &[optical_format], 1);
        let blur_next = pipeline(device, &blur_shader, &[&blur_layout], "fs_next", &[optical_format], 1);
        let mut blur_groups = Vec::new();
        let mut blur_inputs = Vec::new();
        let mut blur_outputs = Vec::new();
        for mip in 1..mip_count {
            let input = mip_view(mip-1);
            blur_groups.push(device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("Glass Gaussian pyramid level"), layout: &blur_layout, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&input) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&depth) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&edges.mask) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&background) },
                wgpu::BindGroupEntry { binding: 5, resource: glass_uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: edges.bounds().as_entire_binding() },
            ] }));
            blur_inputs.push(input);
            blur_outputs.push(mip_view(mip));
        }
        Self { key, bind_group, uniform, glass_uniform, edges, capture_color, receiver_color, background,
            background_ready: false, color, background_capture, blur_first, blur_next, blur_groups,
            blur_layout, blur_inputs, blur_outputs, capture_layout, sampler, depth, capture, capture_receiver, hybrid_capture: None, submerged_capture: None, receiver_only: None, blit,
            water_distance, water_distance_capture, distance_params, distance_group,
            procedural: procedural::Pattern::new(device) }
    }

    /// Disabled recovery keeps only a 1x1 binding placeholder. Toggle allocation
    /// and bindings without rebuilding shaders or resetting optical uniforms.
    pub fn configure_background(&mut self, device: &wgpu::Device, output_layout: &wgpu::BindGroupLayout, enabled: bool) {
        if self.background_ready == enabled { return; }
        self.background = if enabled { background_target(device, self.key.0, self.key.1) }
            else { background_target(device, 1, 1) };
        self.background_ready = enabled;
        self.bind_group = bind(device, output_layout, &self.color, &self.depth, &self.sampler,
            &self.uniform, Some((&self.glass_uniform, &self.edges.field, &self.edges.mask, &self.background)), None);
        self.blur_groups = self.blur_inputs.iter().map(|input| device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Glass Gaussian pyramid recovery bindings"), layout: &self.blur_layout, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(input) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.depth) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&self.edges.mask) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&self.background) },
                wgpu::BindGroupEntry { binding: 5, resource: self.glass_uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: self.edges.bounds().as_entire_binding() },
            ],
        })).collect();
    }

    /// Save the world before opaque models obscure it. Model-behind-glass color
    /// still comes from the later full capture; only foreground holes use this.
    pub fn capture_background(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView, depth: &wgpu::TextureView, ocean: &wgpu::BindGroup) {
        let input = bind(device, &self.capture_layout, scene, depth, &self.sampler, &self.uniform, None,
            Some((&self.procedural.view, &self.procedural.sampler)));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Glass background before opaque models"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.background, depth_slice: None, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })], ..Default::default()
        });
        crate::renderer::tile_probe_scissor(&mut pass, self.key.0, self.key.1);
        pass.set_pipeline(&self.background_capture);
        pass.set_bind_group(0, &input, &[]);
        pass.set_bind_group(1, ocean, &[]);
        pass.draw(0..3, 0..1);
    }

    /// `scissor` is `(x, y, width, height)` in pixels, or `None` for the whole
    /// target. The attachments are still cleared in full (a clear is cheap and a
    /// pass-level clear ignores the scissor anyway); the scissor only keeps the
    /// expensive per-pixel volume/mask work inside the rectangle that something
    /// will actually sample.
    pub fn capture(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView, depth: &wgpu::TextureView, ocean: &wgpu::BindGroup,
        scissor: Option<(u32, u32, u32, u32)>) {
        self.capture_with_receiver(device, encoder, scene, depth, ocean, scissor, false, false, false);
    }

    /// Build the receiver-only capture on first use (`receiver_only_source`).
    pub fn ensure_receiver_only_capture(&mut self, device: &wgpu::Device, ocean_layout: &wgpu::BindGroupLayout) {
        if self.receiver_only.is_some() { return; }
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ocean optical capture, receiver only"),
            source: wgpu::ShaderSource::Wgsl(receiver_only_source(self.key.2 > 1).into()),
        });
        self.receiver_only = Some(pipeline(device, &shader, &[&self.capture_layout, ocean_layout],
            "fs_capture_receiver_only", &[wgpu::TextureFormat::Rgba16Float], 1));
    }

    /// Writes only `receiver_color` (Godot caustics on the opaque scene, before any absorption), the one capture
    /// output the native Blue Water surface reads. The fogged colour and linear depth targets keep their last
    /// contents, so the caller uses this only on frames where nothing else reads them.
    pub fn capture_receiver_only(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView, depth: &wgpu::TextureView, ocean: &wgpu::BindGroup,
        scissor: Option<(u32, u32, u32, u32)>) {
        let pipeline = self.receiver_only.as_ref().expect("ensure_receiver_only_capture");
        let input = bind(device, &self.capture_layout, scene, depth, &self.sampler, &self.uniform, None,
            Some((&self.procedural.view, &self.procedural.sampler)));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Ocean optical capture, receiver only"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.receiver_color, depth_slice: None, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })], ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &input, &[]);
        pass.set_bind_group(1, ocean, &[]);
        if let Some((x, y, width, height)) = scissor {
            if width == 0 || height == 0 {
                return;
            }
            let [x, y, width, height] = crate::renderer::tile_probe_constrain([x, y, width, height], self.key.0, self.key.1);
            pass.set_scissor_rect(x, y, width, height);
        } else {
            crate::renderer::tile_probe_scissor(&mut pass, self.key.0, self.key.1);
        }
        pass.draw(0..3, 0..1);
    }

    /// Only Hybrid mode 4 compiles this shader. Non-Hybrid capture retains its
    /// exact original pipelines, with no wave sampling or shader-cache churn.
    pub fn ensure_hybrid_wave_capture(&mut self, device: &wgpu::Device,
        ocean_layout: &wgpu::BindGroupLayout) {
        if self.hybrid_capture.is_some() { return; }
        let source = shader_source(self.key.2 > 1).replace(
            "const JKA_HYBRID_WAVE: bool = false;",
            "const JKA_HYBRID_WAVE: bool = true;");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("JKA Hybrid wave-aware optics"), source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let color = wgpu::TextureFormat::Rgba16Float;
        let depth = wgpu::TextureFormat::R32Float;
        self.hybrid_capture = Some((
            pipeline(device, &shader, &[&self.capture_layout, ocean_layout],
                "fs_capture", &[color, depth], 1),
            pipeline(device, &shader, &[&self.capture_layout, ocean_layout],
                "fs_capture_receiver", &[color, depth, color], 1),
        ));
    }

    /// Submerged Hybrid mode 4: the eye is in water, so the capture fogs each
    /// pixel over its ray up to the water top over the eye (`Uniform::eye` w)
    /// instead of solving every volume, wave crossing, outline and sky column.
    pub fn ensure_submerged_fog_capture(&mut self, device: &wgpu::Device,
        ocean_layout: &wgpu::BindGroupLayout) {
        if self.submerged_capture.is_some() { return; }
        let source = shader_source(self.key.2 > 1).replace(
            "const JKA_SUBMERGED_FOG: bool = false;",
            "const JKA_SUBMERGED_FOG: bool = true;");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("JKA submerged fog optics"), source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let color = wgpu::TextureFormat::Rgba16Float;
        let depth = wgpu::TextureFormat::R32Float;
        self.submerged_capture = Some((
            pipeline(device, &shader, &[&self.capture_layout, ocean_layout],
                "fs_capture", &[color, depth], 1),
            pipeline(device, &shader, &[&self.capture_layout, ocean_layout],
                "fs_capture_receiver", &[color, depth, color], 1),
        ));
    }

    pub fn capture_with_receiver(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView, depth: &wgpu::TextureView, ocean: &wgpu::BindGroup,
        scissor: Option<(u32, u32, u32, u32)>, receiver: bool, hybrid_wave: bool, submerged_fog: bool) {
        let input = bind(device, &self.capture_layout, scene, depth, &self.sampler, &self.uniform, None,
            Some((&self.procedural.view, &self.procedural.sampler)));
        let views: &[_] = if receiver { &[&self.capture_color, &self.depth, &self.receiver_color] }
            else { &[&self.capture_color, &self.depth] };
        let attachments: Vec<_> = views.iter().map(|view| Some(wgpu::RenderPassColorAttachment {
            view, depth_slice: None, resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
        })).collect();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Ocean optical capture"), color_attachments: &attachments, ..Default::default()
        });
        let submerged = self.submerged_capture.as_ref().filter(|_| submerged_fog);
        pass.set_pipeline(match (hybrid_wave, receiver) {
            _ if submerged.is_some() => {
                let (plain, with_receiver) = submerged.expect("submerged fog pipeline");
                if receiver { with_receiver } else { plain }
            }
            (true, true) => &self.hybrid_capture.as_ref().expect("hybrid wave pipeline").1,
            (true, false) => &self.hybrid_capture.as_ref().expect("hybrid wave pipeline").0,
            (false, true) => &self.capture_receiver,
            (false, false) => &self.capture,
        });
        pass.set_bind_group(0, &input, &[]);
        pass.set_bind_group(1, ocean, &[]);
        if let Some((x, y, width, height)) = scissor {
            if width == 0 || height == 0 {
                return;
            }
            let [x, y, width, height] = crate::renderer::tile_probe_constrain([x, y, width, height], self.key.0, self.key.1);
            pass.set_scissor_rect(x, y, width, height);
        } else {
            crate::renderer::tile_probe_scissor(&mut pass, self.key.0, self.key.1);
        }
        pass.draw(0..3, 0..1);
    }
    /// Only generates on first use of Hybrid animated caustics. There is no
    /// per-frame compute dispatch even when the animation runs at high FPS.
    pub fn ensure_procedural_pattern(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) {
        self.procedural.ensure_generated(device, encoder);
    }

    /// Blurred levels above mip 0; glass_shade clamps its lod to this.
    pub fn blur_levels(&self) -> usize {
        self.blur_groups.len()
    }

    /// Builds mips 1..=`levels` over the rectangle from `edges.locate`.
    pub fn blur(&self, encoder: &mut wgpu::CommandEncoder, levels: usize) {
        for (index, (group, output)) in self.blur_groups.iter().zip(&self.blur_outputs).take(levels).enumerate() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Glass Gaussian pyramid"), color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: output, depth_slice: None, resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })], ..Default::default()
            });
            pass.set_pipeline(if index == 0 { &self.blur_first } else { &self.blur_next });
            pass.set_bind_group(0, group, &[]);
            // The instance index is the output mip level (see glass_blur.wgsl).
            let level = index as u32 + 1;
            pass.draw(0..6, level..level + 1);
        }
    }

    /// Bounded JKA volume path after the original surface has written depth.
    /// The original underwater shader clamps its optical path to this length.
    pub fn capture_water_distance(&self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder,
        scene: &wgpu::TextureView, depth: &wgpu::TextureView, ocean: &wgpu::BindGroup, envelope: [f32; 4]) {
        queue.write_buffer(&self.distance_params, 0, bytemuck::bytes_of(&envelope));
        let input = bind(device, &self.capture_layout, scene, depth, &self.sampler, &self.uniform, None,
            Some((&self.procedural.view, &self.procedural.sampler)));
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Blue Water JKA bounded underwater path"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.water_distance, depth_slice: None, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })], ..Default::default()
        });
        crate::renderer::tile_probe_scissor(&mut pass, self.key.0, self.key.1);
        pass.set_pipeline(&self.water_distance_capture);
        pass.set_bind_group(0, &input, &[]);
        pass.set_bind_group(1, ocean, &[]);
        pass.set_bind_group(2, &self.distance_group, &[]);
        pass.draw(0..3, 0..1);
    }
    pub fn composite(&self, encoder: &mut wgpu::CommandEncoder, color: &wgpu::TextureView, resolve: Option<&wgpu::TextureView>) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Ocean optical composite"), color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color, depth_slice: None, resolve_target: resolve,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })], ..Default::default()
        });
        crate::renderer::tile_probe_scissor(&mut pass, self.key.0, self.key.1);
        pass.set_pipeline(&self.blit);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
fn background_target(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Optional glass world background"),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    }).create_view(&Default::default())
}
fn bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, color: &wgpu::TextureView,
    depth: &wgpu::TextureView, sampler: &wgpu::Sampler, uniform: &wgpu::Buffer,
    glass: Option<(&wgpu::Buffer, &wgpu::TextureView, &wgpu::TextureView, &wgpu::TextureView)>,
    procedural: Option<(&wgpu::TextureView, &wgpu::Sampler)>) -> wgpu::BindGroup {
    let mut entries = vec![
        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(color) },
        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(depth) },
        wgpu::BindGroupEntry { binding: 3, resource: uniform.as_entire_binding() },
    ];
    if let Some((uniform, field, mask, background)) = glass {
        entries.push(wgpu::BindGroupEntry { binding: 4, resource: uniform.as_entire_binding() });
        entries.push(wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(field) });
        entries.push(wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(mask) });
        entries.push(wgpu::BindGroupEntry { binding: 7, resource: wgpu::BindingResource::TextureView(background) });
    }
    if let Some((view, sampler)) = procedural {
        entries.push(wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(view) });
        entries.push(wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(sampler) });
    }
    device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("Ocean optics bindings"), layout,
        entries: &entries,
    })
}
fn pipeline(device: &wgpu::Device, shader: &wgpu::ShaderModule, layouts: &[&wgpu::BindGroupLayout], entry: &str,
    formats: &[wgpu::TextureFormat], samples: u32) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Ocean optics pipeline layout"), bind_group_layouts: &layouts.iter().map(|l| Some(*l)).collect::<Vec<_>>(), immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(entry), layout: Some(&layout),
        vertex: wgpu::VertexState { module: shader, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
        primitive: Default::default(), depth_stencil: None,
        multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
        fragment: Some(wgpu::FragmentState { module: shader, entry_point: Some(entry), compilation_options: Default::default(),
            targets: &formats.iter().map(|&format| Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })).collect::<Vec<_>>() }),
        multiview_mask: None, cache: crate::pipeline_cache::get().as_ref(),
    })
}

/// The capture shader reduced to its `receiver` output: the scene colour times the Godot caustics, which needs only
/// each box's receiver test (`bottom_depth`) and the caustic block. The water spans (`g_spans`, mask and sky spans), the
/// water fraction and the fog only feed the fogged colour, depth and water distance, so they are removed. The kept code
/// is the same text in the same order, so `receiver` is the full shader's.
pub fn receiver_only_source(msaa: bool) -> String {
    let source = shader_source(msaa).replace("\r\n", "\n");
    let cut = |source: String, from: &str, to: &str, with: &str| {
        assert_eq!(source.matches(from).count(), 1, "optics receiver-only anchor changed: {from}");
        let start = source.find(from).unwrap();
        let end = start + source[start..].find(to).unwrap_or_else(|| panic!("optics receiver-only end missing: {to}"));
        format!("{}{}{}", &source[..start], with, &source[end..])
    };
    let source = cut(source, "        let candidate = interval(", "        let receiver_margin", "");
    let source = cut(source, "    if (depth > 0.0) {\n        add_sky_ocean_spans(delta);", "    // Optional low-cost wave-curvature caustics.", "");
    let source = cut(source, "    // BlueWater applies its own column absorption.", "    var out: OpticalSample;",
        "    let receiver_color = vec4<f32>(color, 1.0);\n");
    assert_eq!(source.matches("out.water_distance = ray_length * water_fraction;").count(), 1);
    let source = source.replace("out.water_distance = ray_length * water_fraction;", "out.water_distance = 0.0;");
    format!("{source}\n@fragment fn fs_capture_receiver_only(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{\n    return capture_pixel(p).receiver;\n}}\n")
}

pub fn shader_source(msaa: bool) -> String {
        let bsp = include_str!("../bsp.wgsl");
        let ocean_struct = &bsp[bsp.find("struct OceanRenderSettings {").unwrap()..];
        let ocean_struct = &ocean_struct[..ocean_struct.find("};").unwrap() + 2];
        let source = include_str!("../ocean_optics.wgsl").replace("// OCEAN_STRUCT", ocean_struct);
        let capture_source = source
            .replace("COLOR_DECLARATION", if msaa {
                "@group(0) @binding(0) var scene_color: texture_multisampled_2d<f32>;"
            } else {
                "@group(0) @binding(0) var scene_color: texture_2d<f32>;"
            })
            .replace("DEPTH_DECLARATION", if msaa {
                "@group(0) @binding(2) var scene_depth: texture_depth_multisampled_2d;"
            } else {
                "@group(0) @binding(2) var scene_depth: texture_depth_2d;"
            })
            .replace("CAPTURE_BODY", if msaa {
                r#"let sample_count = textureNumSamples(scene_depth);
    let first_depth = textureLoad(scene_depth, pixel, 0);
    var min_depth = first_depth;
    var max_depth = first_depth;
    for (var i = 1u; i < sample_count; i += 1u) {
        let d = textureLoad(scene_depth, pixel, i32(i));
        min_depth = min(min_depth, d);
        max_depth = max(max_depth, d);
    }
    // Interior pixels take the old one-shade fast path. Only MSAA depth
    // discontinuities need sample-matched optics; this keeps the full-screen
    // water pass close to its previous cost while fixing silhouette halos.
    if (max_depth - min_depth <= 0.00001) {
        return capture_sample(uv, first_depth, textureLoad(scene_color, pixel, 0).rgb);
    }
    var out: OpticalSample;
    out.color = vec4<f32>(0.0);
    out.depth = 1000000.0;
    out.water_distance = 0.0;
    out.receiver = vec4<f32>(0.0);
    for (var i = 0u; i < sample_count; i += 1u) {
        let sample = capture_sample(
            uv,
            textureLoad(scene_depth, pixel, i32(i)),
            textureLoad(scene_color, pixel, i32(i)).rgb,
        );
        out.color += sample.color;
        out.depth = min(out.depth, sample.depth);
        out.water_distance = max(out.water_distance, sample.water_distance);
        out.receiver += sample.receiver;
    }
    out.color = vec4<f32>(out.color.rgb / f32(sample_count), 1.0);
    out.receiver = vec4<f32>(out.receiver.rgb / f32(sample_count), 1.0);
    return out;"#
            } else {
                r#"return capture_sample(
        uv,
        textureLoad(scene_depth, pixel, 0),
        textureLoad(scene_color, pixel, 0).rgb,
    );"#
            });
        capture_source
}

#[cfg(test)]
#[path = "optics/boundary_tests.rs"]
mod boundary_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_surface_selects_its_own_authored_floor() {
        let mut volumes = Uniform::zeroed();
        volumes.params[3] = 2.0;
        volumes.minimum[0] = [0.0,-56.0,0.0,0.0];
        volumes.maximum[0] = [100.0,0.0,100.0,0.0];
        volumes.minimum[1] = [200.0,-4000.0,0.0,0.0];
        volumes.maximum[1] = [300.0,0.0,100.0,0.0];
        let surface = super::super::OceanSurface { plane_height:0.0, minimum:[0.0,0.0], maximum:[100.0,100.0] };
        assert_eq!(volumes.surface_bottom(surface), -56.0);
    }

    #[test]
    fn hybrid_wave_optics_shaders_validate() {
        // The Hybrid specialization samples Blue Water's adapter displacement
        // with no additional binding layout. Validate both MSAA variants.
        for msaa in [false, true] {
            let source = shader_source(msaa).replace(
                "const JKA_HYBRID_WAVE: bool = false;",
                "const JKA_HYBRID_WAVE: bool = true;");
            let module = naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source)));
            naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
                .validate(&module).unwrap_or_else(|error| panic!("{}", error.emit_to_string(&source)));
        }
    }

    #[test]
    fn optics_shaders_validate() {
        let submerged = |msaa| shader_source(msaa).replace("const JKA_SUBMERGED_FOG: bool = false;",
            "const JKA_SUBMERGED_FOG: bool = true;");
        for source in [shader_source(false), shader_source(true), receiver_only_source(false), receiver_only_source(true),
            submerged(false), submerged(true),
            include_str!("../ocean_composite.wgsl").to_owned()] {
            let module = naga::front::wgsl::parse_str(&source).unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
            naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
                .validate(&module).unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
        }
        assert_eq!(std::mem::size_of::<Uniform>(), 2752);
        let procedural = include_str!("optics/procedural.wgsl");
        let module = naga::front::wgsl::parse_str(procedural).unwrap_or_else(|e| panic!("{}", e.emit_to_string(procedural)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module).unwrap_or_else(|e| panic!("{}", e.emit_to_string(procedural)));
    }

    #[test]
    fn invalid_settings_are_finite_and_bounded() {
        let settings = Settings { transparency: -1.0, refraction: f32::INFINITY, ..Default::default() }.sanitize();
        assert_eq!(settings.transparency, 1.0);
        assert_eq!(settings.refraction, 0.1);
    }

    #[test]
    fn cull_distance_follows_transparency_with_a_visibility_floor() {
        let s = Settings { transparency: 2000.0, underwater_cull: 3.0, ..Default::default() };
        assert_eq!(s.cull_distance(), 6000.0);
        // Below 2 absorption distances geometry is still visible: clamp up.
        assert_eq!(Settings { underwater_cull: 0.07, ..s }.cull_distance(), 4000.0);
        assert_eq!(Settings { underwater_cull: 0.0, ..s }.cull_distance(), 0.0);
        // Murkier water culls sooner.
        assert_eq!(Settings { transparency: 500.0, ..s }.cull_distance(), 1500.0);
    }

    #[test]
    #[ignore = "requires a GPU adapter; run explicitly"]
    fn optics_gpu_capture_matches_beer_lambert() {
        futures_lite::future::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance.request_adapter(&Default::default()).await.expect("GPU adapter");
            let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(), ..Default::default()
            }).await.expect("GPU device");
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let ocean_layout = crate::ocean::OceanGpu::create_render_layout(&device);
            let ocean = crate::ocean::OceanGpu::create_inert_render_bind_group(&device, &ocean_layout);
            let layout = layout(&device);
            let extent = wgpu::Extent3d { width: 8, height: 8, depth_or_array_layers: 1 };
            for samples in [1, 4] {
                let mut resources = Resources::new(&device, &layout, &ocean_layout, (8, 8, samples, wgpu::TextureFormat::Rgba8Unorm));
                let texture = |format, samples, usage| device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Optics GPU test target"), size: extent, mip_level_count: 1, sample_count: samples,
                    dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[],
                });
                let color = texture(wgpu::TextureFormat::Rgba8Unorm, 1, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC);
                let color_view = color.create_view(&Default::default());
                let msaa = texture(
                    wgpu::TextureFormat::Rgba8Unorm,
                    samples,
                    wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                );
                let msaa_view = msaa.create_view(&Default::default());
                let depth = texture(wgpu::TextureFormat::Depth32Float, samples, wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING);
                let depth_view = depth.create_view(&Default::default());
                let output = if samples == 1 { &color_view } else { &msaa_view };
                let resolve = (samples > 1).then_some(&color_view);
                // Last pair: a wave reaches above the nominal volume plane.
                // The distance envelope follows it; legacy color is unchanged.
                for (volumes, surface_z, fraction, top, crest, distance_fraction, receiver_depth, open) in [
                    (0, 10.0, 0.0, 10.0, 0.0, 0.0, 0.5, false), (1, 10.0, 1.0, 10.0, 0.0, 1.0, 0.5, false),
                    (2, 10.0, 1.0, 10.0, 0.0, 1.0, 0.5, false), (1, 0.25, 0.5, 10.0, 0.0, 0.5, 0.5, false),
                    (1, 10.0, 0.5, -0.0625, 0.0, 0.5, 0.5, false), (1, 10.0, 0.5, -0.0625, 0.1, 1.0, 0.5, false),
                    (1, 10.0, 1.0, 10.0, 0.0, 1.0, 0.0, false), (1, 10.0, 1.0, 10.0, 0.0, 1.0, 0.0, true),
                    // The camera is under the top but below the finite brush:
                    // neither ordinary nor sky-connected water fogs this ray.
                    (1, 10.0, 0.0, 12.0, 0.0, 0.0, 0.5, false),
                    (1, 10.0, 0.0, 12.0, 0.0, 0.0, 0.5, true),
                    // The specialized submerged pass must also stop at the
                    // floor halfway along this downward ray.
                    (1, 10.0, 0.5, 13.0, 0.0, 0.5, 0.5, false),
                ] {
                    let mut uniform = Uniform::zeroed();
                    uniform.inverse_view_projection = glam::Mat4::IDENTITY.to_cols_array_2d();
                    uniform.forward = [0.0,0.0,1.0,0.0];
                    uniform.eye[3] = top;
                    uniform.fog[3] = 20.0;
                    uniform.params = [20.0,0.0,0.0,volumes as f32];
                    for i in 0..volumes {
                        let bottom = if top == 12.0 { 1.0 } else if top == 13.0 { -0.0625 } else { -10.0 };
                        uniform.minimum[i] = [-10.0,bottom,-10.0,0.0];
                        uniform.maximum[i] = [10.0,top,surface_z,0.0];
                        uniform.forward[3] = bottom;
                    }
                    if open {
                        uniform.sky_info[0] = 1.0;
                        uniform.sky_regions[0].bounds = [-10.0, -10.0, 10.0, 10.0];
                        uniform.sky_regions[0].plane = [top, 1.0, 0.0, uniform.minimum[0][1]];
                        uniform.sky_regions[0].segments[0] = [10.0, -10.0, 10.0, 2.0];
                    }
                    queue.write_buffer(&resources.uniform, 0, bytemuck::bytes_of(&uniform));
                    let mut encoder = device.create_command_encoder(&Default::default());
                    {
                        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: output, depth_slice: None, resolve_target: resolve,
                                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store } })],
                            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &depth_view,
                                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(receiver_depth), store: wgpu::StoreOp::Store }), stencil_ops: None }), ..Default::default()
                        });
                    }
                    let capture_color = if samples == 1 { &color_view } else { &msaa_view };
                    if top == 13.0 { resources.ensure_submerged_fog_capture(&device, &ocean_layout); }
                    resources.capture_with_receiver(&device, &mut encoder, capture_color, &depth_view, &ocean, None, true, false, top == 13.0);
                    resources.capture_water_distance(&device, &queue, &mut encoder, capture_color, &depth_view, &ocean, [top, crest, 0.0, 0.0]);
                    resources.composite(&mut encoder, output, resolve);
                    let readback = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 256*8,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
                    encoder.copy_texture_to_buffer(color.as_image_copy(), wgpu::TexelCopyBufferInfo { buffer: &readback,
                        layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: Some(8) } }, extent);
                    let distance_readback = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 256*8,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
                    encoder.copy_texture_to_buffer(resources.water_distance.texture().as_image_copy(),
                        wgpu::TexelCopyBufferInfo { buffer: &distance_readback,
                            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: Some(8) } }, extent);
                    let receiver_readback = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 256*8,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
                    encoder.copy_texture_to_buffer(resources.receiver_color.texture().as_image_copy(),
                        wgpu::TexelCopyBufferInfo { buffer: &receiver_readback,
                            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: Some(8) } }, extent);
                    queue.submit([encoder.finish()]);
                    readback.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
                    distance_readback.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
                    receiver_readback.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
                    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
                    let bytes = readback.slice(..).get_mapped_range().expect("GPU readback range");
                    let receiver_bytes = receiver_readback.slice(..).get_mapped_range().expect("receiver color readback");
                    let receiver: &[u16] = bytemuck::cast_slice(&receiver_bytes[4*256+4*8..4*256+5*8]);
                    if receiver != &[0x3c00; 4] {
                        panic!("receiver must retain raw white before Godot absorption, samples={samples}, actual={receiver:?}, validation={:?}", scope.pop().await);
                    }
                    let ray = (0.125_f32.powi(2)*2.0 + receiver_depth.powi(2)).sqrt();
                    let distance = ray * fraction;
                    let distances = distance_readback.slice(..).get_mapped_range().expect("bounded length readback");
                    let actual = f32::from_le_bytes(distances[4*256+4*4..4*256+4*4+4].try_into().unwrap());
                    // The inert adapter's map scale is zero: the distance pass
                    // still uses JKA's fixed metre conversion, even without waves.
                    let bounded = if receiver_depth == 0.0 {
                        // This fixture's sky continuation has the same floor
                        // as its pool, so the downward ray exits at Y=-10.
                        200.0_f32.sqrt()
                    } else { ray * distance_fraction } / crate::ocean::OCEAN_UNITS_PER_METER;
                    assert!((actual - bounded).abs() < 1e-3,
                        "samples={samples} volumes={volumes} top={top} open={open}: bounded length {actual}, expected {bounded}");
                    for (channel, ratio) in [38.0/7.5, 38.0/22.0, 1.0].iter().enumerate() {
                        let expected = (-4.321928 * ratio * distance / 20.0).exp2();
                        let actual = bytes[4*256+4*4+channel] as f32 / 255.0;
                        assert!((expected-actual).abs() < 0.008, "samples={samples} volumes={volumes} channel={channel}: {actual} != {expected}");
                    }
                }
            }
            assert!(scope.pop().await.is_none(), "GPU validation error");
        });
    }
}

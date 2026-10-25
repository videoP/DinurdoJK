//! Shared change-triggered lightmap composition: authored-sun removal, indirect
//! tone and static replacement sunlight. Always rebuild from immutable sources.
use crate::renderer::{GpuImage, Renderer};
use wgpu::util::DeviceExt;

#[derive(Clone, Debug, PartialEq)]
struct Composition {
    generation: u64,
    radiance: [f32; 3],
    direction: [f32; 3],
    strength: f32,
    mode: f32,
    adjust: [f32; 4],
    curve: [f32; 4],
}

pub(in crate::renderer) struct LightmapCache {
    pub original: Vec<GpuImage>,
    corrected: Vec<GpuImage>,
    pipeline: wgpu::ComputePipeline,
    pub active: bool,
    /// AO/source writes invalidate the whole composition through this key.
    pub strength: Option<u32>,
    composition: Option<Composition>,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Parameters {
    fit: [f32; 4],
    radiance_strength: [f32; 4],
    adjust: [f32; 4],
    curve: [f32; 4],
    sun: [f32; 4],
    direction_strength: [f32; 4],
}

fn cache_shader() -> String {
    // Both the live fallback and the bake execute the same subtraction/tone
    // functions. There is one compositor regardless of which input changed.
    let world = include_str!("../../bsp.wgsl");
    let tone = world.find("fn baked_indirect_tone(").unwrap();
    let start = world.find("fn baked_file_to_linear(").unwrap();
    let end = world.find("fn sample_authored_baked_sun(").unwrap();
    let shader = format!(
        r#"
struct Settings {{ baked_sun_direction_strength:vec4<f32>, baked_sun_radiance:vec4<f32>, baked_sun_debug_mode:vec4<u32>, baked_indirect_adjust:vec4<f32>, baked_indirect_curve:vec4<f32> }}
var<private> lighting_settings:Settings;
struct VertexOut {{baked_sun_fit:vec4<f32>,world_normal:vec3<f32>}}
struct Parameters {{fit:vec4<f32>,radiance_strength:vec4<f32>,adjust:vec4<f32>,curve:vec4<f32>,sun:vec4<f32>,direction_strength:vec4<f32>}}
@group(0) @binding(0) var source:texture_2d<f32>;
@group(0) @binding(1) var corrected:texture_storage_2d<rgba16float,write>;
@group(0) @binding(2) var<uniform> parameters:Parameters;
@group(0) @binding(3) var sun_flux:texture_2d<f32>;
@group(0) @binding(4) var baked_local_texture:texture_2d_array<f32>;
@group(0) @binding(5) var<storage,read> local_colors:array<vec4<f32>>;
fn baked_luminance(rgb:vec3<f32>)->f32 {{return dot(rgb,vec3(0.2126,0.7152,0.0722));}}
{}
{}
fn source_luxel(at:vec2<i32>)->vec4<f32> {{
    let texel=textureLoad(source,clamp(at,vec2(0),vec2<i32>(textureDimensions(source))-1),0);
    var input:VertexOut; input.baked_sun_fit=parameters.fit;input.world_normal=vec3(0.0,1.0,0.0);
    var rgb=texel.rgb;
    if parameters.fit.z==4.0 {{rgb=subtract_authored_baked_sun(rgb,texel.a,input);}}
    return vec4(rgb,texel.a);
}}
@compute @workgroup_size(8,8)
fn main(@builtin(global_invocation_id) id:vec3<u32>) {{
    let dims=textureDimensions(corrected);if any(id.xy>=dims) {{return;}}
    lighting_settings.baked_sun_direction_strength=vec4(0.0,1.0,0.0,parameters.radiance_strength.w);
    lighting_settings.baked_sun_radiance=vec4(parameters.radiance_strength.rgb,1.0);
    lighting_settings.baked_sun_debug_mode=vec4<u32>(0u);
    lighting_settings.baked_indirect_adjust=parameters.adjust;
    lighting_settings.baked_indirect_curve=parameters.curve;
    // Subtract each original luxel BEFORE resampling, matching normal filtering.
    let at=(vec2<f32>(id.xy)+0.5)*vec2<f32>(textureDimensions(source))/vec2<f32>(dims)-0.5;
    let p=vec2<i32>(floor(at));let f=fract(at);
    let base=mix(mix(source_luxel(p),source_luxel(p+vec2(1,0)),f.x),mix(source_luxel(p+vec2(0,1)),source_luxel(p+vec2(1,1)),f.x),f.y);
    var result=base;
    if parameters.sun.w>0.0 {{
        let incident=max(textureLoad(sun_flux,vec2<i32>(id.xy),0).r,0.0);
        let indirect=baked_indirect_tone(base.rgb);
        if parameters.sun.w<1.5 {{
            result=vec4(indirect+parameters.sun.rgb*incident,incident);
        }} else {{
            // Legacy CSM darkens existing illumination. Store the removable
            // fraction so moving shadows never darken indirect light twice.
            let facing=parameters.direction_strength.w;
            // Flux already includes cosine; the unoccluded facing is stored
            // separately in the second half of the incident image (below).
            let cosine=max(textureLoad(sun_flux,vec2<i32>(id.xy)+vec2(0,i32(dims.y)),0).r,0.0);
            let factor=max(1.0-facing*(cosine-incident),1e-5);
            result=vec4(indirect*factor,clamp(facing*incident/factor,0.0,1.0));
        }}
    }}
    if parameters.curve.z > 0.0 {{
        if parameters.sun.w <= 0.0 {{ result = vec4(baked_indirect_tone(base.rgb),0.0); }}
        var local = vec3(0.0);
        let uv=(vec2<f32>(id.xy)+0.5)/vec2<f32>(dims);
        for (var i=0u;i<u32(parameters.curve.z);i+=1u) {{ local += local_colors[i].rgb*sample_baked_local(uv,i32(i)); }}
        result=vec4(result.rgb+local,result.a);
    }}
    textureStore(corrected,vec2<i32>(id.xy),result);
}}
"#,
        &world[tone..start],
        &world[start..end]
    );
    let helpers = include_str!("lightmap_local.wgsl")
        .split("// Runtime receiver helpers.")
        .next()
        .unwrap();
    shader.replace(
        "@compute @workgroup_size",
        &format!("{helpers}\n@compute @workgroup_size"),
    )
}

fn corrected_images(device: &wgpu::Device, sizes: &[[u32; 2]]) -> Vec<GpuImage> {
    sizes
        .iter()
        .map(|s| {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("composed sunlight lightmap"),
                size: wgpu::Extent3d {
                    width: s[0],
                    height: s[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::STORAGE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            GpuImage {
                _texture: texture,
                view,
            }
        })
        .collect()
}

impl LightmapCache {
    fn new(device: &wgpu::Device, original: &[GpuImage]) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shared lightmap composition"),
            source: wgpu::ShaderSource::Wgsl(cache_shader().into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: wgpu::TextureFormat::Rgba16Float,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("shared lightmap composition"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let sizes: Vec<_> = original
            .iter()
            .map(|i| [i._texture.width(), i._texture.height()])
            .collect();
        Self {
            original: original.to_vec(),
            corrected: corrected_images(device, &sizes),
            pipeline,
            active: false,
            strength: None,
            composition: None,
        }
    }

    #[cfg(test)]
    fn bake(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoding: crate::scene::BakedSunEncoding,
        radiance: [f32; 3],
        strength: f32,
    ) {
        self.compose(
            device,
            queue,
            Some(encoding),
            radiance,
            strength,
            None,
            &[],
            &[],
            &[],
        );
    }

    fn compose(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoding: Option<crate::scene::BakedSunEncoding>,
        radiance: [f32; 3],
        strength: f32,
        composition: Option<&Composition>,
        flux: &[GpuImage],
        local_flux: &[GpuImage],
        colors: &[[f32; 4]],
    ) {
        let sizes: Vec<_> = if composition.is_some() {
            flux.iter()
                .map(|i| [i._texture.width(), i._texture.height() / 2])
                .collect()
        } else {
            self.original
                .iter()
                .map(|i| [i._texture.width(), i._texture.height()])
                .collect()
        };
        if self
            .corrected
            .iter()
            .zip(&sizes)
            .any(|(i, s)| i._texture.width() != s[0] || i._texture.height() != s[1])
        {
            self.corrected = corrected_images(device, &sizes);
        }
        let colors_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("baked local colors"),
            contents: bytemuck::cast_slice(if colors.is_empty() {
                &[[0.0f32; 4]]
            } else {
                colors
            }),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("lightmap composition update"),
        });
        for (index, (original, corrected)) in self.original.iter().zip(&self.corrected).enumerate()
        {
            let fit = encoding.map_or([0.0, 1.0, 0.0, 1.0], |e| {
                [
                    e.exposure,
                    e.gamma,
                    4.0,
                    e.compensate
                        * if original._texture.format().is_srgb() {
                            1.0
                        } else {
                            -1.0
                        },
                ]
            });
            let params = Parameters {
                fit,
                radiance_strength: [radiance[0], radiance[1], radiance[2], strength],
                adjust: composition.map_or([1.0, 1.0, 0.0, 0.0], |c| c.adjust),
                curve: {
                    let mut value = composition.map_or([1.0, 1.0, 0.0, 0.0], |c| c.curve);
                    value[2] = colors.len() as f32;
                    value
                },
                sun: composition.map_or([0.0; 4], |c| {
                    [c.radiance[0], c.radiance[1], c.radiance[2], c.mode]
                }),
                direction_strength: composition.map_or([0.0; 4], |c| {
                    [c.direction[0], c.direction[1], c.direction[2], c.strength]
                }),
            };
            let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::bytes_of(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let fallback_local = original._texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("lightmap composition inputs"),
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(
                            local_flux.get(index).map_or(&fallback_local, |i| &i.view),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: colors_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&original.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&corrected.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(
                            &flux.get(index).unwrap_or(original).view,
                        ),
                    },
                ],
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("compose static sunlight"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(
                corrected._texture.width().div_ceil(8),
                corrected._texture.height().div_ceil(8),
                1,
            );
        }
        queue.submit([encoder.finish()]);
        self.strength = Some(strength.to_bits());
        self.composition = composition.cloned();
    }
}

impl Renderer {
    pub(in crate::renderer) fn baked_sun_cache_active(&self) -> bool {
        self.world
            .as_ref()
            .and_then(|w| w.baked_sun_cache.as_ref())
            .is_some_and(|c| c.active)
    }
    pub(in crate::renderer) fn static_sun_lightmap_active(&self) -> bool {
        self.world
            .as_ref()
            .and_then(|w| w.baked_sun_cache.as_ref())
            .is_some_and(|c| c.active && c.composition.as_ref().is_some_and(|c| c.mode > 0.0))
    }
    /// All callers meet here: sun changes, removal controls, AO uploads and
    /// worker completion only invalidate inputs; warm frames allocate nothing.
    pub(in crate::renderer) fn sync_baked_sun_lightmap_cache(&mut self) {
        let sun = self.active_sun();
        let Some(world) = self.world.as_ref() else {
            return;
        };
        let supported = (self.baked_sun_cache || self.baked_local_lights) && !world.source_map;
        let removal = supported
            && world.baked_sun_encoding.is_some()
            && world.baked_sun_radiance.is_some()
            && self.baked_sun_removal > 0.0
            && self.baked_sun_debug_mode == 0;
        let sun_ready = self.baked_sun_cache
            && world.static_sun.ready()
            && (removal || self.baked_sun_removal <= 0.0 || world.sun.is_none());
        let local_ready = self.baked_local_lights && world.static_sun.local_ready();
        let composition = (supported
            && (sun_ready || local_ready)
            && self.baked_sun_debug_mode == 0)
            .then(|| Composition {
                generation: world.static_sun.generation,
                radiance: sun.color.map(|c| c * sun.intensity.max(0.0) / 255.0),
                direction: sun.direction,
                strength: (self.sun_shadow_strength
                    * (sun.intensity / 250.0).max(0.0).sqrt().clamp(0.0, 1.5))
                .min(1.0),
                mode: if !sun_ready {
                    0.0
                } else if self.sun_lighting {
                    1.0
                } else {
                    2.0
                },
                adjust: [
                    self.baked_indirect_scale,
                    self.baked_indirect_saturation,
                    self.baked_sun_edge_softness,
                    self.baked_sun_direct_limit,
                ],
                curve: [
                    self.baked_indirect_power,
                    self.baked_indirect_highlight_preserve,
                    0.0,
                    0.0,
                ],
            });
        let desired = removal || composition.is_some();
        let strength = if removal { self.baked_sun_removal } else { 0.0 };
        match world.baked_sun_cache.as_ref() {
            Some(c)
                if c.active == desired
                    && c.composition == composition
                    && (!desired || c.strength == Some(strength.to_bits())) =>
            {
                return;
            }
            None if !desired => return,
            _ => {}
        }
        let mut world = self.world.take().unwrap();
        let mut rebound = false;
        if desired {
            let c = world
                .baked_sun_cache
                .get_or_insert_with(|| LightmapCache::new(&self.device, &world.lightmaps));
            c.compose(
                &self.device,
                &self.queue,
                if removal {
                    world.baked_sun_encoding
                } else {
                    None
                },
                world.baked_sun_radiance.unwrap_or([0.0; 3]),
                strength,
                composition.as_ref(),
                &world.static_sun.flux,
                if local_ready {
                    &world.static_sun.local_flux
                } else {
                    &[]
                },
                &if local_ready {
                    world
                        .static_sun
                        .local_lights
                        .iter()
                        .map(|(_, l)| [l.color[0], l.color[1], l.color[2], 1.0])
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                },
            );
            world.lightmaps = c.corrected.clone();
            c.active = true;
            rebound = true;
            println!(
                "[SUN LIGHTMAP] composed {} page(s), removal {:.3}, static sunlight {}, local emitters {}",
                world.lightmaps.len(),
                strength,
                sun_ready, if local_ready {world.static_sun.local_lights.len()} else {0}
            );
        } else if let Some(c) = world.baked_sun_cache.as_mut() {
            world.lightmaps = c.original.clone();
            c.active = false;
            c.composition = None;
            rebound = true;
        }
        if rebound {
            world.local_shadows.selection_position = None;
            self.rebuild_world_surface_bind_groups(&mut world);
        }
        self.world = Some(world);
        self.upload_light_buffer();
        self.update_lighting_settings();
        self.rebuild_frame_plan();
        self.activate_world_pipeline_variant();
        self.sync_runtime_variant_flags();
        self.history_valid = false;
    }
}

#[cfg(test)]

mod tests {
    use super::*;

    #[test]
    fn lightmap_composition_and_receivers_validate() {
        for (name, source) in [
            ("lightmap compositor", cache_shader()),
            (
                "full world",
                crate::renderer::compose_world_shader(include_str!("../../bsp.wgsl")),
            ),
            (
                "lean world",
                crate::renderer::compose_world_shader(include_str!("../../bsp_lean.wgsl")),
            ),
        ] {
            let module = wgpu::naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&source)));
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(&source)));
        }
    }

    #[test]
    #[ignore = "requires a Vulkan GPU"]
    fn lightmap_composition_bakes_sun_without_accumulation_on_gpu() {
        crate::renderer::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance.request_adapter(&Default::default()).await.unwrap();
            let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let original = crate::renderer::upload_texture(
                &device,
                &queue,
                &crate::materials::TextureData {
                    label: "sun composition source".into(),
                    source: None,
                    width: 1,
                    height: 1,
                    rgba: vec![255; 4],
                    rgba16f: None,
                    mip_level_count: 1,
                    clamp: true,
                    srgb: true,
                },
            );
            let mut cache = LightmapCache::new(&device, &[original]);
            // Two independent-resolution sunlight texels over one original texel.
            let texture = device.create_texture_with_data(
                &queue,
                &wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: 2,
                        height: 2,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                bytemuck::cast_slice(&[1.0f32, 0.0, 1.0, 1.0]),
            );
            let view = texture.create_view(&Default::default());
            let flux = vec![GpuImage {
                _texture: texture,
                view,
            }];
            let half = |bits: u16| {
                let sign = if bits & 32768 != 0 { -1.0 } else { 1.0 };
                let e = ((bits >> 10) & 31) as i32;
                let m = (bits & 1023) as f32 / 1024.0;
                sign * if e == 0 {
                    m * 2.0f32.powi(-14)
                } else {
                    (1.0 + m) * 2.0f32.powi(e - 15)
                }
            };
            let local_texture = device.create_texture_with_data(
                &queue,
                &wgpu::TextureDescriptor {
                    label: Some("two independent local sources"),
                    size: wgpu::Extent3d {
                        width: 2,
                        height: 1,
                        depth_or_array_layers: 2,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R32Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                bytemuck::cast_slice(&[0.2f32, 0.4, 0.6, 0.1]),
            );
            let local_view = local_texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
            let local_flux = vec![GpuImage {
                _texture: local_texture,
                view: local_view,
            }];
            let colors = [[1.0f32, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]];
            for (mode, local_enabled) in [
                (1.0, false),
                (1.0, true),
                (0.0, true),
                (2.0, true),
                (1.0, false),
            ] {
                let composition = Composition {
                    generation: 1,
                    radiance: [0.6, 0.4, 0.2],
                    direction: [0.0, -1.0, 0.0],
                    strength: 0.3,
                    mode,
                    adjust: [1.0, 1.0, 0.0, 0.0],
                    curve: [1.0, 1.0, 0.0, 0.0],
                };
                cache.compose(
                    &device,
                    &queue,
                    None,
                    [0.0; 3],
                    0.0,
                    Some(&composition),
                    &flux,
                    if local_enabled { &local_flux } else { &[] },
                    if local_enabled { &colors } else { &[] },
                );
                assert_eq!(cache.corrected[0]._texture.width(), 2);
                let read = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: 256,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                encoder.copy_texture_to_buffer(
                    cache.corrected[0]._texture.as_image_copy(),
                    wgpu::TexelCopyBufferInfo {
                        buffer: &read,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(256),
                            rows_per_image: Some(1),
                        },
                    },
                    wgpu::Extent3d {
                        width: 2,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                );
                queue.submit([encoder.finish()]);
                read.slice(..)
                    .map_async(wgpu::MapMode::Read, |r| r.unwrap());
                device
                    .poll(wgpu::PollType::Wait {
                        submission_index: None,
                        timeout: None,
                    })
                    .unwrap();
                let bytes = read.slice(..).get_mapped_range().unwrap();
                for x in 0..2 {
                    for c in 0..4 {
                        let incident = if x == 0 { 1.0 } else { 0.0 };
                        let mut expected = if c == 3 {
                            if mode == 0.0 {
                                0.0
                            } else if mode == 1.0 {
                                incident
                            } else {
                                0.3 * incident / (1.0 - 0.3 * (1.0 - incident))
                            }
                        } else if mode == 0.0 {
                            1.0
                        } else if mode == 1.0 {
                            1.0 + composition.radiance[c] * incident
                        } else {
                            1.0 - 0.3 * (1.0 - incident)
                        };
                        if local_enabled && c < 3 {
                            expected += match (x, c) {
                                (0, 0) => 0.2,
                                (1, 0) => 0.4,
                                (0, 2) => 0.6,
                                (1, 2) => 0.1,
                                _ => 0.0,
                            };
                        }
                        let offset = x * 8 + c * 2;
                        let actual = half(u16::from_le_bytes([bytes[offset], bytes[offset + 1]]));
                        assert!(
                            (actual - expected).abs() < 0.002,
                            "mode {mode}, pixel {x}, channel {c}: {actual} vs {expected}"
                        );
                    }
                }
            }
            let correction = format!(
                r#"
struct PointLight {{ position_radius:vec4<f32>,color_intensity:vec4<f32>,emitter:vec4<f32>,shape:vec4<f32>,shadow:vec4<f32> }}
struct Settings {{ baked_sun_debug_mode:vec4<u32>,baked_indirect_curve:vec4<f32>,values:vec4<u32> }}
struct VertexOut {{ shell_kind:u32,lightmap_uv:vec2<f32>,world_position:vec3<f32>,world_normal:vec3<f32> }}
var<private> lighting_settings:Settings;var<private> dynamic_lights:array<PointLight,2>;var<private> test_case:u32;
@group(0) @binding(0) var<storage,read_write> output:array<vec4<f32>>;
@group(0) @binding(1) var baked_local_texture:texture_2d_array<f32>;
fn material_flags()->u32 {{return 2u;}} fn material_tc_gen()->u32 {{return 0u;}}
fn dynamic_light_debug_enabled()->bool {{return false;}}
fn lamp_raster_receiver_culling_safe()->bool {{return true;}}
fn baked_local_moving_maybe(light:PointLight,position:vec3<f32>)->bool {{return true;}}
fn sample_local_shadow_visibility(light:PointLight,position:vec3<f32>,normal:vec3<f32>)->f32 {{return 1.0;}}
fn baked_local_moving_visibility(light:PointLight,position:vec3<f32>,normal:vec3<f32>)->f32 {{
    let layer=u32(light.shape.w)-1u;return select(1.0,0.0,(test_case & (1u<<layer))!=0u);
}}
{}
@compute @workgroup_size(1) fn main(@builtin(global_invocation_id) id:vec3<u32>) {{
    test_case=id.x;lighting_settings.baked_sun_debug_mode=vec4(0u,1u,2u,1u);
    lighting_settings.baked_indirect_curve=vec4(1.0,1.0,513.0,0.0);lighting_settings.values.y=2u;
    dynamic_lights[0].color_intensity=vec4(1.0,0.0,0.0,1.0);dynamic_lights[0].shape.w=1.0;dynamic_lights[0].shadow.x=1.0;
    dynamic_lights[1].color_intensity=vec4(0.0,0.0,1.0,1.0);dynamic_lights[1].shape.w=2.0;dynamic_lights[1].shadow.x=2.0;
    dynamic_lights[0].position_radius.w=100.0;dynamic_lights[1].position_radius.w=100.0;
    var input:VertexOut;input.lightmap_uv=vec2(0.25,0.5);
    output[id.x]=vec4(shade_baked_local(vec3(1.2,1.0,1.6),input),1.0);
}}
"#,
                include_str!("lightmap_local.wgsl")
            );
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("actual local moving-shadow correction"),
                source: wgpu::ShaderSource::Wgsl(correction.into()),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: None,
                layout: None,
                module: &module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });
            let output = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let read = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 64,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: output.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&local_flux[0].view),
                    },
                ],
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_compute_pass(&Default::default());
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &group, &[]);
                pass.dispatch_workgroups(4, 1, 1);
            }
            encoder.copy_buffer_to_buffer(&output, 0, &read, 0, 64);
            queue.submit([encoder.finish()]);
            read.slice(..)
                .map_async(wgpu::MapMode::Read, |r| r.unwrap());
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
            let bytes = read.slice(..).get_mapped_range().unwrap();
            for (case, expected) in [
                [1.2f32, 1.0, 1.6],
                [1.0, 1.0, 1.6],
                [1.2, 1.0, 1.0],
                [1.0, 1.0, 1.0],
            ]
            .iter()
            .enumerate()
            {
                for channel in 0..3 {
                    let offset = (case * 4 + channel) * 4;
                    let value = f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
                    assert!(
                        (value - expected[channel]).abs() < 0.0001,
                        "case {case} channel {channel}: {value} vs {}",
                        expected[channel]
                    );
                }
            }
            assert!(scope.pop().await.is_none());
        });
    }

    #[test]
    #[ignore = "requires a Vulkan GPU"]
    fn baked_sun_cache_preserves_source_and_rebuilds_without_accumulation_on_gpu() {
        crate::renderer::block_on(async {
            let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
                backends: wgpu::Backends::VULKAN,
                ..wgpu::InstanceDescriptor::new_without_display_handle()
            });
            let adapter = instance.request_adapter(&Default::default()).await.unwrap();
            let (device, queue) = adapter.request_device(&Default::default()).await.unwrap();
            for exposure in [0.0_f32, 260.0] {
                let sun = if exposure == 0.0 { 140.0 } else { 600.0 };
                let baseline = [40.0_f32, 60.0, 30.0];
                let encode = |raw: [f32; 3]| {
                    let peak = raw.into_iter().fold(0.0_f32, f32::max);
                    raw.map(|v| {
                        if exposure == 0.0 {
                            v / peak.max(255.0)
                        } else {
                            v / peak * (1.0 - (-peak / exposure).exp())
                        }
                    })
                };
                let linear = |v: f32| {
                    if v <= 0.04045 {
                        v / 12.92
                    } else {
                        ((v + 0.055) / 1.055).powf(2.4)
                    }
                };
                let mut rgba = Vec::new();
                for flux in [1.0, 0.0, 0.5, 1.0, 0.0, 0.5] {
                    let rgb = encode(baseline.map(|v| v + sun * flux));
                    rgba.extend(rgb.map(|v| (v * 255.0).round() as u8));
                    rgba.push((flux * 255.0).round() as u8);
                }
                let data = crate::materials::TextureData {
                    label: "sun cache fixture".into(),
                    source: None,
                    width: 3,
                    height: 2,
                    rgba,
                    rgba16f: None,
                    mip_level_count: 1,
                    clamp: true,
                    srgb: true,
                };
                let original = crate::renderer::upload_texture(&device, &queue, &data);
                let mut cache = LightmapCache::new(&device, &[original]);
                let encoding = crate::scene::BakedSunEncoding {
                    exposure,
                    gamma: 1.0,
                    compensate: 1.0,
                    brightness: 1.0,
                    deviance: 0.0,
                    samples: 1,
                };
                // Repeated full/zero/half/full updates must always read the
                // original. The non-multiple-of-eight dimensions test dispatch bounds.
                for strength in [1.0, 0.0, 0.5, 1.0] {
                    cache.bake(&device, &queue, encoding, [sun / 255.0; 3], strength);
                    let readback = device.create_buffer(&wgpu::BufferDescriptor {
                        label: None,
                        size: 512,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    });
                    let mut encoder = device.create_command_encoder(&Default::default());
                    encoder.copy_texture_to_buffer(
                        wgpu::TexelCopyTextureInfo {
                            texture: &cache.corrected[0]._texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        wgpu::TexelCopyBufferInfo {
                            buffer: &readback,
                            layout: wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(256),
                                rows_per_image: Some(2),
                            },
                        },
                        wgpu::Extent3d {
                            width: 3,
                            height: 2,
                            depth_or_array_layers: 1,
                        },
                    );
                    queue.submit([encoder.finish()]);
                    readback
                        .slice(..)
                        .map_async(wgpu::MapMode::Read, |result| result.unwrap());
                    device
                        .poll(wgpu::PollType::Wait {
                            submission_index: None,
                            timeout: None,
                        })
                        .unwrap();
                    let bytes = readback.slice(..).get_mapped_range().unwrap();
                    for index in 0..6 {
                        let flux = data.rgba[index * 4 + 3] as f32 / 255.0;
                        let expected =
                            encode(baseline.map(|v| v + sun * flux * (1.0 - strength))).map(linear);
                        for channel in 0..3 {
                            let offset = index / 3 * 256 + index % 3 * 8 + channel * 2;
                            let bits = u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
                            let exponent = (bits >> 10) & 31;
                            let mantissa = (bits & 1023) as f32 / 1024.0;
                            let actual = if exponent == 0 {
                                mantissa * 2.0_f32.powi(-14)
                            } else {
                                (1.0 + mantissa) * 2.0_f32.powi(exponent as i32 - 15)
                            };
                            assert!(
                                (actual - expected[channel]).abs() < 0.012,
                                "exposure {exposure}, strength {strength}, texel {index}, channel {channel}: {actual} vs {}",
                                expected[channel]
                            );
                        }
                    }
                }
            }
        });
    }
}

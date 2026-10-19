//! Change-triggered sunlight subtraction. Original GPU lightmaps remain immutable;
//! the cached half-float result is filtered normally by the world material shader.
use crate::renderer::{GpuImage, Renderer};
use wgpu::util::DeviceExt;

pub(in crate::renderer) struct LightmapCache {
    pub original: Vec<GpuImage>,
    corrected: Vec<GpuImage>,
    pipeline: wgpu::ComputePipeline,
    pub active: bool,
    /// Invalidated by progressive AO writes as well as changes in removal amount.
    pub strength: Option<u32>,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Parameters {
    fit: [f32; 4],
    radiance_strength: [f32; 4],
}

fn cache_shader() -> String {
    // Execute the SAME per-luxel math as the live world shader. Keeping one
    // source prevents optimizations from silently changing the removal result.
    let world = include_str!("../../bsp.wgsl");
    let start = world
        .find("fn baked_file_to_linear(")
        .expect("baked sun conversion");
    let end = world
        .find("fn sample_authored_baked_sun(")
        .expect("baked sun filtering");
    format!(
        r#"
struct Settings {{ baked_sun_direction_strength: vec4<f32>, baked_sun_radiance: vec4<f32>, baked_sun_debug_mode: vec4<u32> }}
var<private> lighting_settings: Settings;
struct VertexOut {{ baked_sun_fit: vec4<f32>, world_normal: vec3<f32> }}
struct Parameters {{ fit: vec4<f32>, radiance_strength: vec4<f32> }}
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var corrected: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<uniform> parameters: Parameters;
fn baked_luminance(rgb: vec3<f32>) -> f32 {{ return dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722)); }}
{}
@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {{
    if (any(id.xy >= textureDimensions(source))) {{ return; }}
    lighting_settings.baked_sun_direction_strength = vec4<f32>(0.0, 1.0, 0.0, parameters.radiance_strength.w);
    lighting_settings.baked_sun_radiance = vec4<f32>(parameters.radiance_strength.rgb, 1.0);
    lighting_settings.baked_sun_debug_mode = vec4<u32>(0u);
    var input: VertexOut;
    input.baked_sun_fit = parameters.fit;
    input.world_normal = vec3<f32>(0.0, 1.0, 0.0);
    let texel = textureLoad(source, vec2<i32>(id.xy), 0);
    textureStore(corrected, vec2<i32>(id.xy), vec4<f32>(subtract_authored_baked_sun(texel.rgb, texel.a, input), texel.a));
}}
"#,
        &world[start..end]
    )
}

impl LightmapCache {
    fn new(device: &wgpu::Device, original: &[GpuImage]) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("baked sun cache"),
            source: wgpu::ShaderSource::Wgsl(cache_shader().into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("baked sun cache"),
            layout: None,
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let corrected = original
            .iter()
            .map(|image| {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("cached baked sun lightmap"),
                    size: image._texture.size(),
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
            .collect();
        Self {
            original: original.to_vec(),
            corrected,
            pipeline,
            active: false,
            strength: None,
        }
    }

    fn bake(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoding: crate::scene::BakedSunEncoding,
        radiance: [f32; 3],
        strength: f32,
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("baked sun cache update"),
        });
        for (original, corrected) in self.original.iter().zip(&self.corrected) {
            let parameters = Parameters {
                fit: [
                    encoding.exposure,
                    encoding.gamma,
                    4.0,
                    encoding.compensate
                        * if original._texture.format().is_srgb() {
                            1.0
                        } else {
                            -1.0
                        },
                ],
                radiance_strength: [radiance[0], radiance[1], radiance[2], strength],
            };
            let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("baked sun cache parameters"),
                contents: bytemuck::bytes_of(&parameters),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("baked sun cache inputs"),
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &[
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
                ],
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("baked sun lightmap subtraction"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(
                original._texture.width().div_ceil(8),
                original._texture.height().div_ceil(8),
                1,
            );
        }
        queue.submit([encoder.finish()]);
        self.strength = Some(strength.to_bits());
    }
}

impl Renderer {
    pub(in crate::renderer) fn baked_sun_cache_active(&self) -> bool {
        self.world
            .as_ref()
            .and_then(|world| world.baked_sun_cache.as_ref())
            .is_some_and(|cache| cache.active)
    }

    /// Called before frame routing. Warm frames only inspect the settings/key;
    /// they allocate nothing, dispatch nothing, and keep existing bind groups.
    pub(in crate::renderer) fn sync_baked_sun_lightmap_cache(&mut self) {
        let Some(world) = self.world.as_ref() else {
            return;
        };
        let desired =
            self.baked_sun_cache && self.baked_sun_removal > 0.0 && self.baked_sun_debug_mode == 0;
        match world.baked_sun_cache.as_ref() {
            Some(cache)
                if self.baked_sun_cache
                    && cache.active == desired
                    && (!desired || cache.strength == Some(self.baked_sun_removal.to_bits())) =>
            {
                return
            }
            None if !desired || world.baked_sun_encoding.is_none() => return,
            _ => {}
        }
        let Some(mut world) = self.world.take() else {
            return;
        };
        let supported = self.baked_sun_cache
            && world.baked_sun_encoding.is_some()
            && world.baked_sun_radiance.is_some()
            && !world.source_map;
        let desired = supported && self.baked_sun_removal > 0.0 && self.baked_sun_debug_mode == 0;
        let mut rebound = false;
        let mut updated = false;
        if desired {
            let cache = world
                .baked_sun_cache
                .get_or_insert_with(|| LightmapCache::new(&self.device, &world.lightmaps));
            if cache.strength != Some(self.baked_sun_removal.to_bits()) {
                cache.bake(
                    &self.device,
                    &self.queue,
                    world.baked_sun_encoding.unwrap(),
                    world.baked_sun_radiance.unwrap(),
                    self.baked_sun_removal,
                );
                updated = true;
                println!(
                    "[BAKED SUN CACHE] updated {} lightmap(s), removal {:.3}",
                    cache.corrected.len(),
                    self.baked_sun_removal
                );
            }
            if !cache.active {
                world.lightmaps = cache.corrected.clone();
                cache.active = true;
                rebound = true;
            }
        } else {
            if let Some(cache) = world.baked_sun_cache.as_mut().filter(|cache| cache.active) {
                world.lightmaps = cache.original.clone();
                cache.active = false;
                rebound = true;
            }
            if !supported {
                world.baked_sun_cache = None;
            }
        }
        if rebound {
            self.rebuild_world_surface_bind_groups(&mut world);
        }
        self.world = Some(world);
        if rebound || updated {
            self.update_lighting_settings();
            self.rebuild_frame_plan();
            self.sync_runtime_variant_flags();
            self.history_valid = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                            assert!((actual - expected[channel]).abs() < 0.012, "exposure {exposure}, strength {strength}, texel {index}, channel {channel}: {actual} vs {}", expected[channel]);
                        }
                    }
                }
            }
        });
    }
}

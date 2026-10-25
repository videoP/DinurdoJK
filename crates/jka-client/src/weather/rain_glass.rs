//! Fullscreen lens-rain pass, ported from Toadstorm/RainyGlassShader.
//!
//! The shader math and material values are kept in `rain_glass.wgsl`. The two
//! PK3-loaded mask payloads are direct channel-preserving conversions of the upstream
//! `droplets_mask.tif` and `rivulets_mask.tif`: RGBA (including TIFF meta0 as
//! Unity's alpha), linear data, Mitchell-scaled from 2048 to the Unity Standalone
//! importer cap of 1024. The upstream importer uses repeat wrapping and a -100
//! mip bias, so this native port stores only mip 0 and samples it directly.

use crate::pipeline_jobs::{hash_config as pipeline_hash, PipelineJobKey, PipelineJobManager};
use bytemuck::{Pod, Zeroable};
use jka_assets::pk3::AssetSearchPath;
use wgpu::util::DeviceExt;

const MASK_SIZE: u32 = 1024;
const MASK_CHANNELS: u32 = 4;
const MASK_BYTES: usize = (MASK_SIZE * MASK_SIZE * MASK_CHANNELS) as usize;
const DROPLET_MASK_QPATH: &str = "textures/japro/droplets.rgba";
const RIVULET_MASK_QPATH: &str = "textures/japro/rivulets.rgba";
const SHADER: &str = include_str!("rain_glass.wgsl");

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct RainGlassUniform {
    time_amount_up: [f32; 4],
    /// Instantaneous weather wind expressed in lens space, in JKA units/sec:
    /// x = camera-right, y = camera-up, z = toward the lens, w = world speed.
    /// The shader uses this only to deform water that is already present in the
    /// authored masks; it never pans the mask domain itself.
    wind_lens: [f32; 4],
}

pub(crate) struct RainGlassResources {
    _droplet_texture: wgpu::Texture,
    _rivulet_texture: wgpu::Texture,
    droplet_view: wgpu::TextureView,
    rivulet_view: wgpu::TextureView,
    source_texture: wgpu::Texture,
    source_view: wgpu::TextureView,
    source_sampler: wgpu::Sampler,
    mask_sampler: wgpu::Sampler,
    uniform_buffer: wgpu::Buffer,
    bind_group_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    shader: wgpu::ShaderModule,
    bind_group: wgpu::BindGroup,
    pipeline: Option<wgpu::RenderPipeline>,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    amount: f32,
    /// Pipeline-job slot. Instances with different formats must not share
    /// one: a request in the same family+slot evicts the other's job.
    job_slot: u32,
}

impl RainGlassResources {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        assets: &mut AssetSearchPath,
    ) -> Result<Self, String> {
        let droplet_mask = load_mask(assets, DROPLET_MASK_QPATH)?;
        let rivulet_mask = load_mask(assets, RIVULET_MASK_QPATH)?;

        let (droplet_texture, droplet_view) = create_mask_texture(
            device,
            queue,
            "JKA RainyGlass droplet mask",
            &droplet_mask,
        );
        let (rivulet_texture, rivulet_view) = create_mask_texture(
            device,
            queue,
            "JKA RainyGlass rivulet mask",
            &rivulet_mask,
        );

        let source_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("JKA RainyGlass scene sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let mask_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("JKA RainyGlass mask sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            // Upstream has trilinear filtering but mipBias=-100. With the LOD
            // clamped to mip 0, retaining only that mip is the same effective path.
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            lod_min_clamp: 0.0,
            lod_max_clamp: 0.0,
            anisotropy_clamp: 1,
            ..Default::default()
        });
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("JKA RainyGlass settings"),
            contents: bytemuck::bytes_of(&RainGlassUniform::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("JKA RainyGlass bind group layout"),
            entries: &[
                texture_entry(0),
                sampler_entry(1),
                texture_entry(2),
                texture_entry(3),
                sampler_entry(4),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("JKA RainyGlass pipeline layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("JKA RainyGlass WGSL"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let (source_texture, source_view) = create_source_texture(device, width, height, format);
        let bind_group = create_bind_group(
            device,
            &bind_group_layout,
            &source_view,
            &source_sampler,
            &droplet_view,
            &rivulet_view,
            &mask_sampler,
            &uniform_buffer,
        );

        Ok(Self {
            _droplet_texture: droplet_texture,
            _rivulet_texture: rivulet_texture,
            droplet_view,
            rivulet_view,
            source_texture,
            source_view,
            source_sampler,
            mask_sampler,
            uniform_buffer,
            bind_group_layout,
            pipeline_layout,
            shader,
            bind_group,
            pipeline: None,
            width,
            height,
            format,
            amount: 0.0,
            job_slot: 0,
        })
    }

    pub(crate) fn prepare(
        &mut self,
        jobs: &mut PipelineJobManager,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        frame_time: f32,
        amount: f32,
        lens_up_alignment: f32,
        wind_lens: [f32; 4],
    ) {
        if self.width != width || self.height != height || self.format != format {
            let format_changed = self.format != format;
            let (source_texture, source_view) = create_source_texture(device, width, height, format);
            self.source_texture = source_texture;
            self.source_view = source_view;
            self.bind_group = create_bind_group(
                device,
                &self.bind_group_layout,
                &self.source_view,
                &self.source_sampler,
                &self.droplet_view,
                &self.rivulet_view,
                &self.mask_sampler,
                &self.uniform_buffer,
            );
            self.width = width;
            self.height = height;
            self.format = format;
            if format_changed {
                self.pipeline = None;
            }
        }

        self.amount = amount.clamp(0.0, 1.0);
        let uniform = RainGlassUniform {
            time_amount_up: [
                frame_time,
                self.amount,
                lens_up_alignment.abs().clamp(0.0, 1.0),
                0.0,
            ],
            wind_lens,
        };
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniform));

        let config = pipeline_hash(&self.format);
        if self.pipeline.is_none() {
            let key = PipelineJobKey::new("rain-glass", self.job_slot, config);
            if let Some(pipeline) = jobs.take_ready(key) {
                self.pipeline = Some(pipeline);
            } else {
                let device = device.clone();
                let layout = self.pipeline_layout.clone();
                let shader = self.shader.clone();
                let format = self.format;
                jobs.request(key, "RainyGlass lens", move || {
                    create_pipeline(&device, &layout, &shader, format)
                });
            }
        }
    }

    /// Use a separate pipeline-job slot (a second instance, e.g. the main menu's).
    pub(crate) fn set_job_slot(&mut self, slot: u32) {
        self.job_slot = slot;
    }

    pub(crate) fn active(&self) -> bool {
        self.amount > 0.001 && self.pipeline.is_some()
    }

    pub(crate) fn source_view(&self) -> &wgpu::TextureView {
        &self.source_view
    }

    pub(crate) fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
    ) {
        let Some(pipeline) = self.pipeline.as_ref().filter(|_| self.amount > 0.001) else {
            return;
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("JKA RainyGlass lens refraction"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn create_mask_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &'static str,
    rgba: &[u8],
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: MASK_SIZE,
                height: MASK_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Upstream TextureImporter.sRGBTexture == 0: these are data masks.
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        rgba,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_source_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA RainyGlass GrabPass source"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    source_view: &wgpu::TextureView,
    source_sampler: &wgpu::Sampler,
    droplet_view: &wgpu::TextureView,
    rivulet_view: &wgpu::TextureView,
    mask_sampler: &wgpu::Sampler,
    uniform_buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("JKA RainyGlass bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(source_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(source_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(droplet_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(rivulet_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(mask_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: uniform_buffer.as_entire_binding(),
            },
        ],
    })
}

fn load_mask(assets: &mut AssetSearchPath, qpath: &str) -> Result<Vec<u8>, String> {
    let asset = assets
        .read(qpath, MASK_BYTES.saturating_add(1))
        .map_err(|error| format!("failed to read {qpath}: {error}"))?
        .ok_or_else(|| format!("missing {qpath}"))?;
    if asset.bytes.len() != MASK_BYTES {
        return Err(format!(
            "invalid {qpath}: expected {MASK_BYTES} RGBA8 bytes, got {}",
            asset.bytes.len()
        ));
    }
    Ok(asset.bytes)
}

fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    shader: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("JKA RainyGlass lens pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: crate::pipeline_cache::get().as_ref(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weather::rain::layout_tests::wgsl_struct_layout;
    use std::mem::size_of;

    #[test]
    fn rainy_glass_assets_match_unity_standalone_import_size() {
        assert_eq!(MASK_BYTES, 1024 * 1024 * 4);
        assert_eq!(
            DROPLET_MASK_QPATH,
            "textures/japro/droplets.rgba"
        );
        assert_eq!(
            RIVULET_MASK_QPATH,
            "textures/japro/rivulets.rgba"
        );
    }

    #[test]
    fn packaged_rainy_glass_masks_have_expected_size() {
        let assets_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let mut assets = AssetSearchPath::open_search_dirs([assets_dir])
            .expect("RainyGlass package directory must open");
        for qpath in [DROPLET_MASK_QPATH, RIVULET_MASK_QPATH] {
            let asset = assets
                .read(qpath, MASK_BYTES.saturating_add(1))
                .expect("RainyGlass mask read must succeed")
                .expect("RainyGlass mask must be packaged");
            assert_eq!(asset.bytes.len(), MASK_BYTES);
        }
    }

    #[test]
    fn rainy_glass_wgsl_parses_and_validates() {
        let module = wgpu::naga::front::wgsl::parse_str(SHADER)
            .unwrap_or_else(|error| panic!("{}", error.emit_to_string(SHADER)));
        wgpu::naga::valid::Validator::new(
            wgpu::naga::valid::ValidationFlags::all(),
            wgpu::naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .expect("RainyGlass WGSL must validate");
    }

    #[test]
    fn rain_glass_uniform_matches_wgsl_layout() {
        let (size, _) = wgsl_struct_layout(SHADER, "RainGlassSettings");
        assert_eq!(size as usize, size_of::<RainGlassUniform>());
    }
}

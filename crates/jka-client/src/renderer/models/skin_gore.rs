//! GPU resources for the original SkinGore damage-mask presentation.
//! Unlike the renderer's immutable material cache, these 64x64 maps update
//! in place on new hit events and are reclaimed when the corresponding model
//! disappears. The stock Ghoul2 pipeline/layout is never changed.
use std::{collections::{HashMap, HashSet}, sync::Arc};
use crate::renderer::{upload_texture, Ghoul2SkinGore};

struct MaskGpu {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    probe: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    revision: u64,
}

pub(in crate::renderer) struct SkinGoreGpu {
    pub layout: wgpu::BindGroupLayout,
    pub pipeline_layout: wgpu::PipelineLayout,
    pub pipeline: Option<wgpu::RenderPipeline>,
    _detail: crate::renderer::GpuImage,
    _metallic: crate::renderer::GpuImage,
    sampler: wgpu::Sampler,
    clamp_sampler: wgpu::Sampler,
    fallback_probe: wgpu::TextureView,
    masks: HashMap<Arc<str>, MaskGpu>,
    active: HashSet<Arc<str>>,
}
impl SkinGoreGpu {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue,
        camera: &wgpu::BindGroupLayout, texture: &wgpu::BindGroupLayout,
        skin: &wgpu::BindGroupLayout,
    ) -> Option<Self> {
        let art = crate::cgame::skin_gore::assets()?;
        let image_entry = |binding, view_dimension| wgpu::BindGroupLayoutEntry {
            binding, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float {filterable:true},
                view_dimension, multisampled:false }, count:None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None,
        };
        let d2 = wgpu::TextureViewDimension::D2;
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("SkinGore damage, material and reflection probe"),
            entries: &[image_entry(0, d2), image_entry(1, d2), image_entry(2, d2), sampler_entry(3),
                image_entry(10, wgpu::TextureViewDimension::Cube), sampler_entry(11)],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label:Some("SkinGore Ghoul2 overlay pipeline layout"),
            bind_group_layouts:&[Some(camera),Some(texture),Some(skin),Some(&layout)],
            immediate_size:0,
        });
        let fallback_probe = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("SkinGore empty reflection probe"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 6 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        }).create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::Cube),
            ..Default::default()
        });
        Some(Self {
            layout, pipeline_layout, pipeline:None,
            _detail:upload_texture(device,queue,&art.detail),
            _metallic:upload_texture(device,queue,&art.metallic),
            sampler:device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u:wgpu::AddressMode::Repeat,
                address_mode_v:wgpu::AddressMode::Repeat,
                address_mode_w:wgpu::AddressMode::Repeat,
                min_filter:wgpu::FilterMode::Linear,
                mag_filter:wgpu::FilterMode::Linear,
                mipmap_filter:wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            }),
            clamp_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                min_filter: wgpu::FilterMode::Linear,
                mag_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            }),
            fallback_probe,
            masks:HashMap::new(),active:HashSet::new(),
        })
    }
    pub fn begin_frame(&mut self) { self.active.clear(); }
    pub fn finish_frame(&mut self) { self.masks.retain(|key,_| self.active.contains(key)); }
    pub fn has(&self,key:&str)->bool { self.masks.contains_key(key) }
    pub fn bind_group(&self,key:&str)->Option<&wgpu::BindGroup> { self.masks.get(key).map(|s| &s.bind_group) }
    pub fn update(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &Ghoul2SkinGore,
        reflection_probe: Option<&wgpu::TextureView>,
    ) {
        self.active.insert(Arc::clone(&source.key));
        let probe = reflection_probe.unwrap_or(&self.fallback_probe);
        let bind = |view: &wgpu::TextureView| device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("SkinGore wound and reflection inputs"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&self._detail.view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self._metallic.view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 10, resource: wgpu::BindingResource::TextureView(probe) },
                wgpu::BindGroupEntry { binding: 11, resource: wgpu::BindingResource::Sampler(&self.clamp_sampler) },
            ],
        });
        if let Some(old) = self.masks.get_mut(&source.key) {
            if old.revision != source.revision {
                write_damage(queue,&old.texture,&source.pixels);
                old.revision = source.revision;
            }
            if &old.probe != probe {
                old.bind_group = bind(&old.view);
                old.probe = probe.clone();
            }
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label:Some("SkinGore persistent 64x64 UV damage"),
            size:wgpu::Extent3d {width:64,height:64,depth_or_array_layers:1},
            mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,
            format:wgpu::TextureFormat::Rgba8Unorm,
            usage:wgpu::TextureUsages::TEXTURE_BINDING|wgpu::TextureUsages::COPY_DST,
            view_formats:&[],
        });
        write_damage(queue,&texture,&source.pixels);
        let view=texture.create_view(&Default::default());
        let bind_group = bind(&view);
        self.masks.insert(Arc::clone(&source.key), MaskGpu {
            texture, view, probe: probe.clone(), bind_group, revision: source.revision,
        });
    }
}
fn write_damage(queue:&wgpu::Queue,texture:&wgpu::Texture,pixels:&[u8]) {
    if pixels.len() != 64*64*4 {return;}
    queue.write_texture(wgpu::TexelCopyTextureInfo {
        texture,mip_level:0,origin:wgpu::Origin3d::ZERO,aspect:wgpu::TextureAspect::All,
    },pixels,wgpu::TexelCopyBufferLayout {
        offset:0,bytes_per_row:Some(64*4),rows_per_image:Some(64),
    },wgpu::Extent3d {width:64,height:64,depth_or_array_layers:1});
}

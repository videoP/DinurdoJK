//! Post targets.
use crate::renderer::{BloomPyramid, DofTarget, TemporalSsaoHistory, TemporalSsrHistory};

pub(in crate::renderer) fn create_temporal_ssao_history(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> TemporalSsaoHistory {
    // Bevy's SSAO follow-up design calls out half-resolution rendering with a
    // bilateral upsample. Keep the temporal history at half resolution as well:
    // it cuts both AO sampling cost and history bandwidth to one quarter.
    let half_width = width.div_ceil(2).max(1);
    let half_height = height.div_ceil(2).max(1);
    let textures: [wgpu::Texture; 2] = std::array::from_fn(|_| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("JKA half-resolution temporal SSAO history"),
            size: wgpu::Extent3d {
                width: half_width,
                height: half_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    });
    let views = std::array::from_fn(|index| {
        textures[index].create_view(&wgpu::TextureViewDescriptor::default())
    });
    TemporalSsaoHistory {
        _textures: textures,
        views,
    }
}

pub(in crate::renderer) fn create_temporal_ssr_history(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> TemporalSsrHistory {
    let half_width = width.div_ceil(2).max(1);
    let half_height = height.div_ceil(2).max(1);
    let radiance_textures: [wgpu::Texture; 2] = std::array::from_fn(|_| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("JKA half-resolution temporal SSR radiance"),
            size: wgpu::Extent3d {
                width: half_width,
                height: half_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    });
    let radiance_views = std::array::from_fn(|index| {
        radiance_textures[index].create_view(&wgpu::TextureViewDescriptor::default())
    });
    let depth_textures: [wgpu::Texture; 2] = std::array::from_fn(|_| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("JKA half-resolution temporal SSR depth"),
            size: wgpu::Extent3d {
                width: half_width,
                height: half_height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    });
    let depth_views = std::array::from_fn(|index| {
        depth_textures[index].create_view(&wgpu::TextureViewDescriptor::default())
    });
    TemporalSsrHistory {
        _radiance_textures: radiance_textures,
        radiance_views,
        _depth_textures: depth_textures,
        depth_views,
    }
}

pub(in crate::renderer) fn create_bloom_pyramid(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> BloomPyramid {
    let mut w = width.max(1);
    let mut h = height.max(1);
    let sizes: [(u32, u32); 3] = std::array::from_fn(|_| {
        w = w.div_ceil(2).max(1);
        h = h.div_ceil(2).max(1);
        (w, h)
    });
    let textures: [wgpu::Texture; 3] = std::array::from_fn(|level| {
        let (width, height) = sizes[level];
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(match level {
                0 => "JKA bloom half-resolution",
                1 => "JKA bloom quarter-resolution",
                _ => "JKA bloom eighth-resolution",
            }),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    });
    let views: [wgpu::TextureView; 3] =
        std::array::from_fn(|level| textures[level].create_view(&Default::default()));
    BloomPyramid {
        _textures: textures,
        views,
    }
}

pub(in crate::renderer) fn create_dof_target(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> DofTarget {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA Gaussian DOF horizontal target"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    DofTarget {
        _texture: texture,
        view,
    }
}

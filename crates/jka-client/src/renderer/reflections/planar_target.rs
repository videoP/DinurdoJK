//! Planar capture precision is independent of the main scene's SDR/HDR mode.

// Lakes reserve alpha=-1 for sky and add their selected environment there.
// UNORM targets clamp that marker to zero, losing sky behind additive effects.
pub(crate) const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

pub(crate) fn color_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    layers: u32,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("JKA planar reflection color array"),
        size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: layers },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

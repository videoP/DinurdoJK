//! Post color lut.
use crate::renderer::{color_lut, ColorLutPreset, Path, Renderer};
use wgpu::util::DeviceExt;

pub(in crate::renderer) fn create_color_lut_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    preset: ColorLutPreset,
    strength: f32,
    split_toning: crate::color_grading::SplitToningSettings,
    gamma: f32,
    base: &Path,
    game: Option<&Path>,
) -> (wgpu::Texture, wgpu::TextureView) {
    let (size, data) =
        color_lut::build_graded_rgba8(preset, strength, split_toning, gamma, base, game);
    upload_color_lut_texture(device, queue, size, &data)
}

fn upload_color_lut_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: u32,
    data: &[u8],
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("JKA color grading 3D LUT"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: size,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            // The OxiMedia presets are display-referred numeric RGB values. Do
            // not use an sRGB texture here: that would decode the LUT itself.
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        data,
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("JKA color grading 3D LUT view"),
        dimension: Some(wgpu::TextureViewDimension::D3),
        ..Default::default()
    });
    (texture, view)
}

impl Renderer {
    /// Returns true only when the view changes and LUT bind groups need updating.
    pub(in crate::renderer) fn rebuild_color_grading_lut(&mut self) -> bool {
        let (size, data) = color_lut::build_graded_rgba8(
            self.color_lut_preset,
            self.color_lut_strength,
            self.split_toning,
            self.output_gamma(),
            &self.base,
            self.detail_texture_game.as_deref(),
        );
        if self._color_lut_texture.width() == size {
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self._color_lut_texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size * 4),
                    rows_per_image: Some(size),
                },
                wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: size,
                },
            );
            return false;
        }
        let (texture, view) = upload_color_lut_texture(&self.device, &self.queue, size, &data);
        self._color_lut_texture = texture;
        self.color_lut_view = view;
        true
    }
}

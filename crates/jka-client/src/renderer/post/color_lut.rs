//! Post color lut.
use crate::renderer::{color_lut, ColorLutPreset, Path, Renderer};
use wgpu::util::DeviceExt;

pub(in crate::renderer) fn create_color_lut_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    preset: ColorLutPreset,
    strength: f32,
    color_grade: crate::color_grading::ColorGradeSettings,
    split_toning: crate::color_grading::SplitToningSettings,
    gamma: f32,
    base: &Path,
    game: Option<&Path>,
) -> (wgpu::Texture, wgpu::TextureView) {
    let (size, data) =
        color_lut::build_graded_rgba8(
            preset, strength, color_grade, split_toning, gamma, base, game,
        );
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

/// Trilinearly sample the pre-existing 3D grading cube on the CPU. This
/// reproduces the UNORM8 sampling before combining it with JK2's output curve;
/// the GPU only samples the final cube once when fusion is enabled.
fn sample_rgba8_cube(size: u32, data: &[u8], coord: [f32; 3]) -> [f32; 3] {
    let max_index = size - 1;
    let f = coord.map(|c| c.clamp(0.0, 1.0) * max_index as f32);
    let lo = f.map(|c| c.floor() as u32);
    let hi = lo.map(|c| (c + 1).min(max_index));
    let frac = std::array::from_fn::<_, 3, _>(|i| f[i] - lo[i] as f32);
    let mut out = [0.0; 3];
    for z in 0..2 {
        for y in 0..2 {
            for x in 0..2 {
                let at = [if x == 0 { lo[0] } else { hi[0] },
                          if y == 0 { lo[1] } else { hi[1] },
                          if z == 0 { lo[2] } else { hi[2] }];
                let weight = (if x == 0 { 1.0 - frac[0] } else { frac[0] })
                    * (if y == 0 { 1.0 - frac[1] } else { frac[1] })
                    * (if z == 0 { 1.0 - frac[2] } else { frac[2] });
                let offset = (at[0] + size * (at[1] + size * at[2])) as usize * 4;
                for channel in 0..3 {
                    out[channel] += weight * data[offset + channel] as f32 * (1.0 / 255.0);
                }
            }
        }
    }
    out
}

/// Build output = Grading(JK2Gamma(scene)) on CPU, rather than doing three
/// 1D gamma samples plus one 3D grading sample in every display fragment.
///
/// Store a quadratic input grid (shader addresses it with sqrt), providing
/// denser interpolation near black. RGBA16F avoids quantizing the gamma curve
/// and creative grade back to RGBA8, which can otherwise band in shadows.
fn fused_jk2_rgba16f(
    source_size: u32, source: &[u8], gamma: f32, overbright: u8, strength: f32,
) -> Vec<u8> {
    let size = color_lut::LUT_SIZE;
    let mut output = Vec::with_capacity((size * size * size * 8) as usize);
    let strength = strength.clamp(0.0, 1.0);
    for b in 0..size {
        for g in 0..size {
            for r in 0..size {
                let mapped = [r, g, b].map(|i| {
                    let x = i as f32 / (size - 1) as f32;
                    crate::renderer::brightness_settings::jk2_output_transfer(x * x, gamma, overbright)
                });
                let graded = sample_rgba8_cube(source_size, source, mapped);
                for channel in 0..3 {
                    let value = mapped[channel] + (graded[channel] - mapped[channel]) * strength;
                    output.extend_from_slice(&crate::materials::f32_to_f16_bits(value).to_le_bytes());
                }
                output.extend_from_slice(&crate::materials::f32_to_f16_bits(1.0).to_le_bytes());
            }
        }
    }
    output
}

impl Renderer {
    /// Keep the exact gamma -> lens/rain -> creative grade order whenever
    /// those intermediate effects are active. Their color depends on gamma,
    /// so they cannot be baked into a static 3D LUT without changing behavior.
    /// Otherwise JK2 SDR grading can use a single 3D lookup.
    pub(in crate::renderer) fn should_fuse_jk2_output_lut(&self) -> bool {
        self.jk2_sdr_color()
            && self.color_lut_effective_strength > 0.001
            && self.chromatic_aberration_strength <= 0.001
            && (self.weather.rain.fast_post || !self.weather.rain.enabled
                || self.weather.rain.wet_grade <= 0.001)
    }

    /// Returns true if GPU view/format/size changed and bind groups must update.
    /// This runs only for cold grade/gamma/mode changes, not every frame.
    pub(in crate::renderer) fn rebuild_color_grading_lut(&mut self) -> bool {
        let (size, original) = color_lut::build_graded_rgba8(
            self.color_lut_preset,
            self.color_lut_strength,
            self.color_grade,
            self.split_toning,
            self.output_gamma(),
            &self.base,
            self.detail_texture_game.as_deref(),
        );
        let fused = self.should_fuse_jk2_output_lut();
        let (size, format, data) = if fused {
            (
                color_lut::LUT_SIZE,
                wgpu::TextureFormat::Rgba16Float,
                fused_jk2_rgba16f(size, &original, self.output_gamma(),
                    self.overbright_bits, self.color_lut_effective_strength),
            )
        } else {
            (size, wgpu::TextureFormat::Rgba8Unorm, original)
        };
        self.jk2_gamma_baked = fused;
        let bytes_per_texel = if fused { 8 } else { 4 };
        if self._color_lut_texture.width() == size
            && self._color_lut_texture.format() == format
        {
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
                    bytes_per_row: Some(size * bytes_per_texel),
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
        let (texture, view) = if fused {
            let texture = self.device.create_texture_with_data(
                &self.queue,
                &wgpu::TextureDescriptor {
                    label: Some("JK2 fused gamma and color grade 3D LUT"),
                    size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: size },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D3,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &data,
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("JK2 fused grade LUT view"),
                dimension: Some(wgpu::TextureViewDimension::D3),
                ..Default::default()
            });
            (texture, view)
        } else {
            upload_color_lut_texture(&self.device, &self.queue, size, &data)
        };
        self._color_lut_texture = texture;
        self.color_lut_view = view;
        true
    }
}

#[cfg(test)]
mod jk2_fusion_tests {
    use super::*;
    use crate::renderer::brightness_settings::jk2_output_transfer;

    #[test]
    fn fusion_precomposes_gamma_and_strength_in_one_cube() {
        let base = color_lut::identity_rgba8(33);
        let lut = fused_jk2_rgba16f(33, &base, 1.0, 1, 0.5);
        assert_eq!(lut.len(), 33 * 33 * 33 * 8);
        // An identity grade should retain the JK2 transfer at any strength.
        let x = 0.25f32;
        let value = jk2_output_transfer(x, 1.0, 1);
        assert!(value > x);
        // Verify a representative texel contains the CPU-composed result,
        // including the original color cube's UNORM8 quantization.
        let i = 16usize;
        let x = (i as f32 / 32.0).powi(2);
        let expected = jk2_output_transfer(x, 1.0, 1);
        let ofs = (i * 8) as usize;
        let got = u16::from_le_bytes([lut[ofs], lut[ofs + 1]]);
        let graded = sample_rgba8_cube(33, &base, [expected; 3]);
        let want = crate::materials::f32_to_f16_bits((expected + graded[0]) * 0.5);
        assert_eq!(got, want);
    }
    #[test]
    fn identity_fused_cube_uses_high_precision_texels() {
        let base = color_lut::identity_rgba8(33);
        let lut = fused_jk2_rgba16f(33, &base, 2.0, 0, 1.0);
        let i = 8usize;
        let x = (i as f32 / 32.0).powi(2);
        let expected = jk2_output_transfer(x, 2.0, 0);
        let ofs = i * 8;
        let got = u16::from_le_bytes([lut[ofs], lut[ofs + 1]]);
        let graded = sample_rgba8_cube(33, &base, [expected; 3]);
        let want = crate::materials::f32_to_f16_bits(graded[0]);
        assert_eq!(got, want);
    }
}

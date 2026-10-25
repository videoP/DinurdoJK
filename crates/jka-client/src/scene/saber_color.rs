//! Once-per-map reduced-resolution linear albedo for enhanced saber scorching.
//! No full-resolution CPU texture retention; no readback at impact time.
use crate::materials::TextureData;

/// 32x32 blurred thumbnail, suitable for a *local* average rather than a
/// whole-material average. The UVs come from the actual BSP triangles.
pub(super) fn mark_color_thumbnail(
    image: &TextureData,
    stage_color: [f32; 3],
) -> Option<jka_assets::bsp::MarkColorTexture> {
    const SIDE: usize = 32;
    let w = image.width as usize;
    let h = image.height as usize;
    if w == 0 || h == 0 || image.rgba.len() < w.checked_mul(h)?.checked_mul(4)? {
        return None;
    }
    let mut pixels = Vec::with_capacity(SIDE * SIDE);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let mut total = [0.0f32; 3];
            let mut weight = 0.0f32;
            // Four stratified texel samples from inside each thumbnail texel.
            // Unlike a global mean, this preserves painted borders and color
            // patches while suppressing the high-frequency texture grain.
            for sy in [0.25, 0.75] {
                for sx in [0.25, 0.75] {
                    let px = (((x as f32 + sx) / SIDE as f32) * w as f32).floor() as usize;
                    let py = (((y as f32 + sy) / SIDE as f32) * h as f32).floor() as usize;
                    let off = (py.min(h - 1) * w + px.min(w - 1)) * 4;
                    let alpha = image.rgba[off + 3] as f32 / 255.0;
                    if alpha < 0.06 { continue; }
                    for channel in 0..3 {
                        let v = image.rgba[off + channel] as f32 / 255.0;
                        let linear = if image.srgb {
                            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
                        } else { v };
                        total[channel] += linear * stage_color[channel] * alpha;
                    }
                    weight += alpha;
                }
            }
            pixels.push(if weight > 0.0 {
                total.map(|value| (value / weight).clamp(0.0, 1.0))
            } else { [0.5; 3] });
        }
    }
    Some(jka_assets::bsp::MarkColorTexture {
        width: SIDE,
        height: SIDE,
        pixels,
        clamp: image.clamp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_two_local_colors_instead_of_a_global_average() {
        let mut rgba = vec![255; 64 * 64 * 4];
        for y in 0..64 { for x in 0..32 {
            rgba[(y * 64 + x) * 4] = 30;
            rgba[(y * 64 + x) * 4 + 1] = 80;
            rgba[(y * 64 + x) * 4 + 2] = 160;
        } }
        let image = TextureData {
            label: "test".into(), source: None, width: 64, height: 64,
            rgba, rgba16f: None, mip_level_count: 1, clamp: false, srgb: true,
        };
        let t = mark_color_thumbnail(&image, [1.0; 3]).unwrap();
        assert!(t.pixels[8 * 32 + 4][2] > t.pixels[8 * 32 + 4][0]);
        assert!(t.pixels[8 * 32 + 24][0] > 0.95);
    }
}

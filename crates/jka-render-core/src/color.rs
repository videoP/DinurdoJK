//! Color helpers.

/// sRGB-encoded color to linear.
pub fn linear_color(rgb: [f32; 3]) -> [f32; 3] {
    rgb.map(|c| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) })
}

/// Default underwater fog color (sRGB) shared by ocean optics and rivers.
pub const UNDERWATER_FOG_COLOR: [f32; 3] = [20.0 / 255.0, 40.0 / 255.0, 50.0 / 255.0];

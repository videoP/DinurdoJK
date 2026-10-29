//! Authored water appearance reduced once at upload, without GPU readback.
//! Source images are analyzed at upload; only FFT data shades promoted water.
use crate::{
    materials::{BlendFactor, TextureData},
    scene::{BlendMode, DrawBatch, GpuVertex},
};
use jka_assets::shader::{AlphaGen, RgbGen, Shader, TcGen, TcMod};

/// Ordinary promoted water should never collapse into an almost-flat sheet.
pub const DEFAULT_PROMOTED_AMPLITUDE: f32 = 10.0;
pub const MIN_PROMOTED_AMPLITUDE: f32 = 10.0;
/// Keep a small coherent Gerstner component so authored tcMod scroll can still
/// steer the promoted swell while the GodotOceanWaves FFT field dominates.
pub const DEFAULT_PROMOTED_FFT_BLEND: f32 = 0.95;

/// Constant displacement offsets are not waves. An authored geometric wave
/// takes precedence over the image heuristic, but promotion still enforces the
/// minimum visible ocean amplitude.
pub fn deformation_amplitude(shader: &Shader) -> Option<f32> {
    let wave = shader.vertex_wave.map_or(0.0, |w| w.wave.amplitude.abs());
    let bulge = shader
        .bulge
        .filter(|b| !b.is_static())
        .map_or(0.0, |b| b.height.abs());
    let amplitude = wave.max(bulge);
    (amplitude.is_finite() && amplitude > 0.0).then_some(amplitude)
}

pub fn amplitude_from_shader(shader: &Shader) -> f32 {
    deformation_amplitude(shader).map_or(DEFAULT_PROMOTED_AMPLITUDE, |a| {
        a.clamp(MIN_PROMOTED_AMPLITUDE, 16384.0)
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveHeuristic {
    pub variation: f32,
    pub amplitude: f32,
    pub chop: f32,
    pub foam: f32,
    pub wavelength: f32,
}

/// Bounded sampling of the original image, before its mip chain erases detail.
/// Color variation is a hint about the painted sea state, not a height map.
pub fn texture_variation(texture: &TextureData) -> f32 {
    let pixels = (texture.width as usize * texture.height as usize).min(texture.rgba.len() / 4);
    if pixels == 0 {
        return 0.0;
    }
    let count = pixels.min(4096);
    let mut sum = 0.0f64;
    let mut squared = 0.0f64;
    for i in 0..count {
        // Jitter within each stratum to avoid aliasing repeating textures
        // (a fixed stride can sample only dark squares of a checkerboard).
        let start = i * pixels / count;
        let end = (i + 1) * pixels / count;
        let mut hash = (i as u32).wrapping_add(0x9e3779b9);
        hash = (hash ^ (hash >> 16)).wrapping_mul(0x85ebca6b);
        hash = (hash ^ (hash >> 13)).wrapping_mul(0xc2b2ae35);
        hash ^= hash >> 16;
        let offset = (start + hash as usize % (end - start).max(1)) * 4;
        let p = &texture.rgba[offset..offset + 4];
        let luma = (0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64) / 255.0;
        sum += luma;
        squared += luma * luma;
    }
    let mean = sum / count as f64;
    (((squared / count as f64 - mean * mean).max(0.0).sqrt() * 4.0) as f32).clamp(0.0, 1.0)
}

pub fn wave_heuristic(variation: f32, deformation: Option<f32>) -> WaveHeuristic {
    let v = if variation.is_finite() {
        variation.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let amplitude = deformation
        .filter(|a| a.is_finite())
        .unwrap_or(8.0 + 24.0 * v)
        .clamp(MIN_PROMOTED_AMPLITUDE, 16384.0);
    // Deforms can imply a stronger sea than the painted texture alone.
    let activity = v.max((amplitude / 64.0).clamp(0.0, 1.0));
    WaveHeuristic {
        variation: v,
        amplitude,
        chop: 0.08 + 0.5 * activity,
        foam: 0.05 + 0.95 * activity,
        wavelength: 256.0 + 512.0 * activity,
    }
}

fn base_uv_scroll_velocity(mods: &[TcMod]) -> Option<[f32; 2]> {
    // final_uv = linear * base_uv + velocity * time + constant
    let mut linear = [[1.0f32, 0.0], [0.0, 1.0]];
    let mut velocity = [0.0f32; 2];
    let mut has_scroll = false;
    for modifier in mods {
        match *modifier {
            TcMod::Scroll(x, y) => {
                velocity[0] += x;
                velocity[1] += y;
                has_scroll |= x.abs() + y.abs() > 1.0e-6;
            }
            TcMod::Scale(x, y) => {
                linear[0][0] *= x;
                linear[0][1] *= x;
                linear[1][0] *= y;
                linear[1][1] *= y;
                velocity[0] *= x;
                velocity[1] *= y;
            }
            TcMod::Transform(v) => {
                let m = [[v[0], v[2]], [v[1], v[3]]];
                let old = linear;
                linear = [
                    [
                        m[0][0] * old[0][0] + m[0][1] * old[1][0],
                        m[0][0] * old[0][1] + m[0][1] * old[1][1],
                    ],
                    [
                        m[1][0] * old[0][0] + m[1][1] * old[1][0],
                        m[1][0] * old[0][1] + m[1][1] * old[1][1],
                    ],
                ];
                velocity = [
                    m[0][0] * velocity[0] + m[0][1] * velocity[1],
                    m[1][0] * velocity[0] + m[1][1] * velocity[1],
                ];
            }
            TcMod::Rotate(_) | TcMod::Stretch(_) => return None,
            TcMod::Turb { .. } => {}
        }
    }
    if !has_scroll {
        return None;
    }

    let det = linear[0][0] * linear[1][1] - linear[0][1] * linear[1][0];
    if !det.is_finite() || det.abs() < 1.0e-8 {
        return None;
    }
    // Quake samples base_uv + scroll*time; visible texture phase therefore
    // travels in the opposite direction in the unmodified base UVs.
    Some([
        -(linear[1][1] * velocity[0] - linear[0][1] * velocity[1]) / det,
        -(-linear[1][0] * velocity[0] + linear[0][0] * velocity[1]) / det,
    ])
}

fn swell_direction_from_base_uv_velocity(
    vertices: &[GpuVertex],
    base_velocity: [f32; 2],
) -> Option<f32> {
    for triangle in vertices.chunks_exact(3) {
        let du1 = [
            triangle[1].uv[0] - triangle[0].uv[0],
            triangle[1].uv[1] - triangle[0].uv[1],
        ];
        let du2 = [
            triangle[2].uv[0] - triangle[0].uv[0],
            triangle[2].uv[1] - triangle[0].uv[1],
        ];
        let uv_det = du1[0] * du2[1] - du2[0] * du1[1];
        if !uv_det.is_finite() || uv_det.abs() < 1.0e-8 {
            continue;
        }
        // Coefficients in the triangle's two world-space edge vectors.
        let c0 = (base_velocity[0] * du2[1] - du2[0] * base_velocity[1]) / uv_det;
        let c1 = (du1[0] * base_velocity[1] - base_velocity[0] * du1[1]) / uv_det;
        let world = [
            (triangle[1].position[0] - triangle[0].position[0]) * c0
                + (triangle[2].position[0] - triangle[0].position[0]) * c1,
            (triangle[1].position[2] - triangle[0].position[2]) * c0
                + (triangle[2].position[2] - triangle[0].position[2]) * c1,
        ];
        let length = world[0].hypot(world[1]);
        if length.is_finite() && length > 1.0e-6 {
            // OceanAuthoring direction maps to render XZ as [cos(a), -sin(a)].
            return Some(
                (-world[1])
                    .atan2(world[0])
                    .to_degrees()
                    .rem_euclid(360.0),
            );
        }
    }
    None
}

/// Recover the visible world-space travel direction of an authored `tcMod scroll`.
///
/// Quake stages sample `uv + scroll * time`, so the painted feature itself moves
/// in the opposite UV direction. We first undo affine tcMods around the scroll,
/// then use the face's actual base-UV mapping to turn that UV velocity into
/// renderer XZ motion. This avoids assuming that the texture's U/V axes line up
/// with world X/Z. Dynamic `tcMod rotate` has no fixed heading, so those stages
/// deliberately fall back to the map/shared swell direction.
pub fn swell_direction_from_stage(source: &DrawBatch, vertices: &[GpuVertex]) -> Option<f32> {
    if !matches!(source.tc_gen, TcGen::Base) {
        return None;
    }
    let base_velocity = base_uv_scroll_velocity(&source.tc_mods)?;
    let slice = &vertices[source.vertices.start as usize..source.vertices.end as usize];
    swell_direction_from_base_uv_velocity(slice, base_velocity)
}

#[derive(Clone, Debug, PartialEq)]
pub struct PromotedOcean {
    pub index: usize,
    pub name: String,
    pub surface: super::OceanSurface,
    pub heuristic: WaveHeuristic,
    pub color: [f32; 3],
    pub roughness: Option<f32>,
    /// Geometric deformation as authored, without the ocean's minimum-height
    /// heuristic. A flat Quake pond must not inherit a 10-unit ocean swell.
    pub source_deformation: Option<f32>,
    /// Actual authored alpha, only for standard source-alpha blending. Additive,
    /// screen and opaque stages do not encode optical transparency in alpha.
    pub source_alpha: Option<f32>,
    /// World-space swell heading recovered from the authored water stage's
    /// tcMod scroll, when its base UV mapping gives us a stable direction.
    pub swell_direction: Option<f32>,
}

// The source image contains baked highlights and foam. Treat its hue as a
// color cue, not its brightness as the water body color. Applied only to
// automatically promoted surfaces; manual pool colors remain untouched.
fn promoted_water_color(color: [f32; 3]) -> [f32; 3] {
    let peak = color.iter().copied().fold(0.0_f32, f32::max);
    let floor = color.iter().copied().fold(1.0_f32, f32::min);
    let saturation = if peak > 1.0e-6 { (peak - floor) / peak } else { 0.0 };
    let target_saturation = (saturation * 1.35).min(1.0);
    let scale = if saturation > 1.0e-6 { target_saturation / saturation } else { 1.0 };
    color.map(|channel| (0.60 * (peak + (channel - peak) * scale)).clamp(0.0, 1.0))
}

impl PromotedOcean {
    pub fn settings(&self, mut shared: super::OceanSettings) -> super::OceanSettings {
        // Promotion supplies only what the map knows: the base amplitude, the
        // water colour (below), an authored roughness and the swell heading.
        // Wavelength, speed, choppiness, wind chop, foam, spray and roughness
        // follow the amplitude through the ocean links (see `surface_settings`).
        shared.authored.amplitude = self.heuristic.amplitude;
        if let Some(direction) = self.swell_direction {
            shared.authored.direction = direction;
        }
        // Keep the global normal-strength setting rather than weakening promoted water.
        shared.water_color = promoted_water_color(self.color);
        shared.roughness = self.roughness.unwrap_or(shared.roughness);
        shared.bounds = None;
        shared
    }
    pub fn label(&self) -> String {
        let x = (self.surface.minimum[0] + self.surface.maximum[0]) * 0.5;
        let y = -(self.surface.minimum[1] + self.surface.maximum[1]) * 0.5;
        format!(
            "Pool {} / {} / ({x:.0}, {y:.0}, {:.0})",
            self.index + 1,
            self.name,
            self.surface.plane_height
        )
    }
}

/// Local controls are map-local overrides. Simulation textures/resolution and
/// underwater optics remain shared, and are clearly grouped in the UI.
pub fn surface_settings(
    shared: super::OceanSettings,
    descriptor: &PromotedOcean,
    local: Option<super::OceanSettings>,
) -> super::OceanSettings {
    let mut settings = local.unwrap_or_else(|| descriptor.settings(shared));
    settings.backend = shared.backend;
    settings.blue_water_waves = shared.blue_water_waves;
    settings.hybrid_fft_blend = shared.hybrid_fft_blend;
    settings.hybrid_swell = shared.hybrid_swell;
    settings.blue_water_optics = shared.blue_water_optics;
    settings.map_size = shared.map_size;
    settings.simulate_when_hidden = shared.simulate_when_hidden;
    settings.updates_per_second = shared.updates_per_second;
    settings.dynamic_update_rates = shared.dynamic_update_rates;
    settings.wind_foam_sync = shared.wind_foam_sync;
    settings.mesh_quality = shared.mesh_quality;
    settings.cascades = shared.cascades;
    settings.optics = shared.optics;
    settings.wind = shared.wind;
    settings.authored.seed = shared.authored.seed;
    settings.authored.foam_lifetime = shared.authored.foam_lifetime;
    settings.bounds = None;
    settings.authored.amplitude = settings.authored.amplitude.max(MIN_PROMOTED_AMPLITUDE);
    // Locked controls follow this pool's amplitude, whether it is automatic or
    // a local override. A shader-authored roughness stays authoritative.
    settings.links = shared.links;
    settings.apply_links(descriptor.roughness.is_some());
    settings.sanitize()
}

pub fn linear_to_srgb(color: [f32; 3]) -> [f32; 3] {
    color.map(|c| {
        if c <= 0.0031308 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        }
    })
}

/// Stable shader ids; zero in ocean_waves.z means the physical ocean default,
/// otherwise z = source factor + 1 and w = destination factor.
pub fn blend_factors(blend: BlendMode) -> [f32; 2] {
    fn id(factor: BlendFactor) -> f32 {
        match factor {
            BlendFactor::Zero => 0.0,
            BlendFactor::One => 1.0,
            BlendFactor::SrcColor => 2.0,
            BlendFactor::OneMinusSrcColor => 3.0,
            BlendFactor::SrcAlpha => 4.0,
            BlendFactor::OneMinusSrcAlpha => 5.0,
            BlendFactor::DstColor => 6.0,
            BlendFactor::OneMinusDstColor => 7.0,
            BlendFactor::DstAlpha => 8.0,
            BlendFactor::OneMinusDstAlpha => 9.0,
            BlendFactor::SrcAlphaSaturate => 10.0,
        }
    }
    match blend {
        BlendMode::Opaque => [0.0; 2],
        BlendMode::Custom(src, dst) => [id(src) + 1.0, id(dst)],
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Appearance {
    pub color: [f32; 4],
    pub material: [f32; 4],
    pub waves: [f32; 4],
    pub heuristic: WaveHeuristic,
    /// Mean source color with baked stage brightness, for blend-mode transparency.
    pub blend_color: [f32; 4],
}

fn texture_mean(texture: &TextureData) -> [f32; 4] {
    let mut offset = 0;
    let mut width = texture.width.max(1);
    let mut height = texture.height.max(1);
    for _ in 1..texture.mip_level_count.max(1) {
        offset += (width * height * 4) as usize;
        width = (width / 2).max(1);
        height = (height / 2).max(1);
    }
    let Some(bytes) = texture
        .rgba
        .get(offset..offset + (width * height * 4) as usize)
    else {
        return [1.0; 4];
    };
    let mut mean = [0.0; 4];
    for pixel in bytes.chunks_exact(4) {
        let rgb = [pixel[0], pixel[1], pixel[2]].map(|c| c as f32 / 255.0);
        let rgb = if texture.srgb {
            super::optics::linear_color(rgb)
        } else {
            rgb
        };
        for i in 0..3 {
            mean[i] += rgb[i];
        }
        mean[3] += pixel[3] as f32 / 255.0;
    }
    mean.map(|c| c / (width * height) as f32)
}

pub fn from_stage(
    source: &DrawBatch,
    vertices: &[GpuVertex],
    textures: &[TextureData],
) -> Appearance {
    let mean = source
        .texture
        .and_then(|i| textures.get(i))
        .map(texture_mean)
        .unwrap_or([1.0; 4]);
    let mut color = mean;
    let slice = &vertices[source.vertices.start as usize..source.vertices.end as usize];
    let mut vertex = [0.0; 4];
    for v in slice {
        for i in 0..4 {
            vertex[i] += v.color[i];
        }
    }
    vertex = vertex.map(|c| c / slice.len().max(1) as f32);
    let mut tint = source.color;
    match source.rgb_gen {
        RgbGen::Vertex | RgbGen::ExactVertex => {
            // Preserve vertex chromaticity. Baked brightness is supplied by the
            // ocean lighting path, rather than charged a second time here.
            let peak = vertex[0].max(vertex[1]).max(vertex[2]).max(0.001);
            for i in 0..3 {
                tint[i] *= vertex[i] / peak;
            }
        }
        RgbGen::OneMinusVertex => {
            for i in 0..3 {
                tint[i] *= 1.0 - vertex[i];
            }
        }
        _ => {}
    }
    match source.alpha_gen {
        AlphaGen::Vertex => tint[3] *= vertex[3],
        AlphaGen::OneMinusVertex => tint[3] *= 1.0 - vertex[3],
        AlphaGen::Identity if matches!(source.rgb_gen, RgbGen::Vertex) => tint[3] *= vertex[3],
        _ => {}
    }
    for i in 0..4 {
        color[i] = (color[i] * tint[i]).clamp(0.0, 1.0);
    }
    // The original blend stage already carries baked vertex brightness. Keep
    // that strength: alpha=1 in a JPEG does not make an additive/screen stage opaque.
    let mut stage_color = source.color;
    match source.rgb_gen {
        RgbGen::Vertex | RgbGen::ExactVertex => {
            for i in 0..3 {
                stage_color[i] *= vertex[i];
            }
        }
        RgbGen::OneMinusVertex => {
            for i in 0..3 {
                stage_color[i] *= 1.0 - vertex[i];
            }
        }
        _ => {}
    }
    match source.alpha_gen {
        AlphaGen::Vertex => stage_color[3] *= vertex[3],
        AlphaGen::OneMinusVertex => stage_color[3] *= 1.0 - vertex[3],
        AlphaGen::Identity if matches!(source.rgb_gen, RgbGen::Vertex) => {
            stage_color[3] *= vertex[3]
        }
        _ => {}
    }
    let blend = blend_factors(source.pipeline.blend);
    let blend_color = std::array::from_fn(|i| mean[i] * stage_color[i]);
    let reflectance = source.specular_reflectance.unwrap_or([-1.0; 3]);
    let variation = source
        .texture
        .and_then(|i| textures.get(i))
        .map_or(0.0, texture_variation);
    let heuristic = wave_heuristic(
        variation,
        source.water_deformed.then_some(source.water_amplitude),
    );
    Appearance {
        color,
        material: [
            reflectance[0],
            reflectance[1],
            reflectance[2],
            source.roughness_override.unwrap_or(-1.0),
        ],
        // Amplitude is now set directly in the selected surface's render
        // uniform; no hidden multiplier attenuates the user's slider.
        waves: [1.0, heuristic.amplitude, blend[0], blend[1]],
        heuristic,
        blend_color,
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    fn image(width: u32, height: u32, rgba: Vec<u8>) -> TextureData {
        TextureData {
            label: String::new(),
            source: None,
            width,
            height,
            rgba: (rgba).into(),
            rgba16f: None,
            mip_level_count: 1,
            clamp: false,
            srgb: true,
        }
    }

    pub(in crate::ocean) fn pool(index: usize, variation: f32) -> PromotedOcean {
        PromotedOcean {
            index,
            name: format!("water{index}"),
            surface: super::super::OceanSurface {
                plane_height: 100.0,
                minimum: [0.0, 0.0],
                maximum: [1024.0, 1024.0],
            },
            heuristic: wave_heuristic(variation, None),
            color: [0.1, 0.2, 0.3],
            roughness: Some(0.4),
            source_deformation: None, source_alpha: None,
            swell_direction: None,
        }
    }

    fn vertex(position: [f32; 3], uv: [f32; 2]) -> GpuVertex {
        GpuVertex {
            position,
            uv,
            lightmap_uv: [0.0; 2],
            normal: [0.0, 1.0, 0.0],
            color: [1.0; 4],
            alpha_cutoff: 0.0,
        }
    }

    #[test]
    fn promoted_water_scroll_uses_actual_world_texture_axes() {
        let vertices = [
            vertex([0.0, 0.0, 0.0], [0.0, 0.0]),
            vertex([64.0, 0.0, 0.0], [1.0, 0.0]),
            vertex([0.0, 0.0, 64.0], [0.0, 1.0]),
        ];
        let u = base_uv_scroll_velocity(&[TcMod::Scroll(1.0, 0.0)]).unwrap();
        let v = base_uv_scroll_velocity(&[TcMod::Scroll(0.0, 1.0)]).unwrap();
        let u_direction = swell_direction_from_base_uv_velocity(&vertices, u).unwrap();
        let v_direction = swell_direction_from_base_uv_velocity(&vertices, v).unwrap();
        assert!((u_direction - 180.0).abs() < 0.001);
        assert!((v_direction - 90.0).abs() < 0.001);
    }

    #[test]
    fn promoted_water_scroll_respects_tcmod_scale_order() {
        assert_eq!(
            base_uv_scroll_velocity(&[TcMod::Scale(2.0, 1.0), TcMod::Scroll(1.0, 0.0)]),
            Some([-0.5, 0.0])
        );
        assert_eq!(
            base_uv_scroll_velocity(&[TcMod::Scroll(1.0, 0.0), TcMod::Scale(2.0, 1.0)]),
            Some([-1.0, 0.0])
        );
    }

    #[test]
    fn promoted_water_defaults_to_fft_dominant_cross_sea() {
        let shared = super::super::OceanSettings::default();
        let settings = pool(0, 0.0).settings(shared);
        assert_eq!(settings.authored.amplitude, MIN_PROMOTED_AMPLITUDE);
        // Promotion supplies the amplitude and colour only; the cross-sea blend
        // is the shared (upstream, pure FFT) value.
        assert_eq!(settings.authored.slosh, shared.authored.slosh);
        assert_eq!(settings.authored.wavelength, shared.authored.wavelength);
        assert_eq!(settings.authored.steepness, shared.authored.steepness);
    }

    #[test]
    fn promoted_water_follows_its_own_amplitude_through_the_links() {
        let shared = super::super::OceanSettings::default();
        let calm = surface_settings(shared, &pool(0, 0.0), None);
        // The shader-authored roughness (pool() sets one) is kept, the rest is derived.
        assert_eq!(calm.roughness, 0.4);
        let d = super::super::authoring::derive_from_amplitude(calm.authored.amplitude);
        assert_eq!(calm.authored.wavelength, d.wavelength);
        assert_eq!(calm.authored.foam, d.foam);
        assert!(calm.authored.wavelength < shared.authored.wavelength);
        // An unlocked control keeps the value the pool started with.
        let mut unlocked = shared;
        unlocked.links &= !super::super::links::WAVELENGTH;
        assert_eq!(surface_settings(unlocked, &pool(0, 0.0), None).authored.wavelength, shared.authored.wavelength);
    }

    #[test]
    fn amplitude_links_equal_upstream_at_the_default_sea_and_scale_monotonically() {
        use super::super::authoring::derive_from_amplitude as derive;
        let d = derive(128.0);
        assert_eq!((d.wavelength, d.speed, d.steepness, d.wind_chop, d.foam, d.spray, d.roughness),
            (1400.0, 180.0, 0.65, 1.0, 1.0, 1.0, 0.2));
        let (small, large) = (derive(10.0), derive(400.0));
        assert!(small.roughness < d.roughness && d.roughness < large.roughness);
        assert!(small.wavelength < d.wavelength && d.wavelength < large.wavelength);
        assert!(small.foam < d.foam && small.steepness < d.steepness);
        let mut defaults = super::super::OceanSettings::default();
        defaults.apply_links(false);
        assert_eq!(defaults, super::super::OceanSettings::default());
    }

    #[test]
    fn promoted_water_texture_variation_increases_waves_without_flat_calm_water() {
        let calm = image(256, 256, vec![96; 256 * 256 * 4]);
        let mut checker = Vec::new();
        for i in 0..256 * 256 {
            let v = if i % 2 == 0 { 0 } else { 255 };
            checker.extend([v, v, v, 255]);
        }
        let busy = image(256, 256, checker);
        let calm_score = texture_variation(&calm);
        let busy_score = texture_variation(&busy);
        assert_eq!(calm_score, 0.0);
        assert!(
            busy_score > 0.9,
            "sampling must not alias a repeated pattern"
        );
        let a = wave_heuristic(calm_score, None);
        let b = wave_heuristic(busy_score, None);
        assert!(a.amplitude >= MIN_PROMOTED_AMPLITUDE);
        assert!(b.amplitude > a.amplitude && b.chop > a.chop && b.foam > a.foam);
        assert_eq!(texture_variation(&image(0, 0, Vec::new())), 0.0);
    }

    #[test]
    fn promoted_water_deformation_overrides_image_amplitude() {
        assert_eq!(wave_heuristic(1.0, Some(2.0)).amplitude, MIN_PROMOTED_AMPLITUDE);
        assert_eq!(wave_heuristic(0.0, Some(80.0)).amplitude, 80.0);
        assert_eq!(
            wave_heuristic(1.0, Some(0.1)).amplitude,
            MIN_PROMOTED_AMPLITUDE
        );
        assert!(wave_heuristic(0.0, Some(80.0)).foam > wave_heuristic(0.0, None).foam);
    }

    #[test]
    fn promoted_water_overrides_are_independent_with_shared_quality_and_a_ripple_floor() {
        let mut shared = super::super::OceanSettings::default();
        let a = pool(0, 0.0);
        let b = pool(1, 1.0);
        let mut local = a.settings(shared);
        local.authored.amplitude = 64.0;
        local.water_color = [0.4, 0.5, 0.6];
        shared.map_size = 128;
        shared.authored.foam_lifetime = 8.0;
        let tuned = surface_settings(shared, &a, Some(local));
        assert_eq!(tuned.authored.amplitude, 64.0);
        assert_eq!(tuned.water_color, local.water_color);
        assert_eq!(tuned.map_size, 128);
        assert_eq!(tuned.authored.foam_lifetime, 8.0);
        assert_eq!(tuned.roughness, 0.4);
        assert_eq!(
            surface_settings(shared, &b, None).authored.amplitude,
            b.heuristic.amplitude
        );
        local.authored.amplitude = 0.0;
        assert_eq!(
            surface_settings(shared, &a, Some(local)).authored.amplitude,
            MIN_PROMOTED_AMPLITUDE
        );
    }

    #[test]
    fn texture_tint_uses_linear_rgb_and_unmodified_alpha() {
        let image = TextureData {
            label: String::new(),
            source: None,
            width: 2,
            height: 1,
            rgba: (vec![255, 128, 0, 64, 255, 128, 0, 64, 255, 128, 0, 64]).into(),
            rgba16f: None,
            mip_level_count: 2,
            clamp: false,
            srgb: true,
        };
        let mean = texture_mean(&image);
        assert_eq!(mean[0], 1.0);
        assert!((mean[1] - 0.21586).abs() < 0.0001);
        assert_eq!(mean[2], 0.0);
        assert!((mean[3] - 64.0 / 255.0).abs() < 0.0001);
    }
}

//! Live glass styles. BSP and model paths share the same GPU transmission math
//! and the existing optical scene capture; style changes never rebuild a PSO.
use bytemuck::{Pod, Zeroable};
use std::sync::RwLock;
pub(crate) mod edges;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Bevy,
    Frosted,
    Liquid,
    Ocean,
}

impl Style {
    pub fn name(self) -> &'static str {
        match self {
            Self::Bevy => "bevy",
            Self::Frosted => "frosted",
            Self::Liquid => "liquid",
            Self::Ocean => "ocean",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "bevy" | "clear" | "0" => Some(Self::Bevy),
            "frosted" | "1" => Some(Self::Frosted),
            "liquid" | "2" => Some(Self::Liquid),
            "ocean" | "3" => Some(Self::Ocean),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum EdgeProfile {
    Convex,
    Squircle,
    Concave,
    Lip,
}
impl EdgeProfile {
    pub fn name(self) -> &'static str {
        match self {
            Self::Convex => "convex",
            Self::Squircle => "squircle",
            Self::Concave => "concave",
            Self::Lip => "lip",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "convex" | "0" => Some(Self::Convex),
            "squircle" | "1" => Some(Self::Squircle),
            "concave" | "2" => Some(Self::Concave),
            "lip" | "3" => Some(Self::Lip),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub style: Style,
    pub blur: f32,
    pub distortion: f32,
    pub dispersion: f32,
    pub roughness: f32,
    pub thickness: f32,
    pub ior: f32,
    pub tint_strength: f32,
    pub reflection: f32,
    pub tint: [f32; 3],
    pub edge_profile: EdgeProfile,
    pub foreground_guard: f32,
    pub frost: f32,
    pub opacity: f32,
    pub effect_strength: f32,
    pub edge_width: f32,
    pub edge_strength: f32,
    pub max_displacement: f32,
    pub magnification: f32,
    pub rim_strength: f32,
    pub rim_width: f32,
    pub rim_softness: f32,
    pub rim_angle: f32,
    pub edge_blur: f32,
    pub edge_blur_mode: f32,
    pub waviness: f32,
    pub animation_speed: f32,
    pub rim_saturation: f32,
    pub rim_type: f32,
    pub dispersion_base: f32,
    pub background_recovery: f32,
}

impl Settings {
    pub const fn preset(style: Style) -> Self {
        let (blur, distortion, dispersion, roughness, thickness) = match style {
            Style::Bevy => (0.0, 1.0, 0.0, 0.12, 6.0),
            Style::Frosted => (8.0, 1.0, 0.0, 0.65, 8.0),
            Style::Liquid => (2.5, 1.4, 0.06, 0.18, 18.0),
            Style::Ocean => (1.0, 0.7, 0.025, 0.12, 8.0),
        };
        Self {
            style,
            blur,
            distortion,
            dispersion,
            roughness,
            thickness,
            ior: 1.5,
            tint_strength: 0.12,
            reflection: if matches!(style, Style::Liquid) {
                0.25
            } else {
                1.0
            },
            tint: [1.0; 3],
            frost: if matches!(style, Style::Liquid) {
                0.65
            } else {
                1.0
            },
            opacity: 0.0,
            effect_strength: 1.0,
            edge_width: 24.0,
            edge_strength: if matches!(style, Style::Liquid) {
                1.0
            } else {
                0.0
            },
            edge_profile: EdgeProfile::Convex,
            foreground_guard: 1.0,
            max_displacement: 32.0,
            magnification: 0.0,
            rim_strength: if matches!(style, Style::Liquid) {
                0.3
            } else {
                0.0
            },
            rim_width: 2.0,
            rim_softness: 2.0,
            rim_angle: 60.0,
            edge_blur: 0.0,
            edge_blur_mode: 0.0,
            waviness: if matches!(style, Style::Ocean) {
                1.0
            } else {
                0.0
            },
            animation_speed: if matches!(style, Style::Ocean) {
                0.15
            } else {
                0.0
            },
            rim_saturation: 1.0,
            rim_type: 1.0,
            dispersion_base: 1.0,
            background_recovery: 0.0,
        }
    }
    pub fn needs_edges(self) -> bool {
        self.effect_strength > 0.0
            && ((self.edge_width > 0.0 && (self.edge_strength > 0.0 || self.edge_blur > 0.0))
                || self.rim_strength > 0.0
                || self.magnification != 0.0
                || (self.foreground_guard > 0.0 && self.distortion > 0.0))
    }
    pub fn needs_mask(self) -> bool {
        self.needs_edges() || self.needs_blur()
    }
    pub fn needs_background(self) -> bool {
        self.background_recovery > 0.5
            && self.effect_strength > 0.0
            && (self.distortion > 0.0
                || self.edge_strength > 0.0
                || self.magnification != 0.0
                || self.needs_blur())
    }
    pub fn needs_blur(self) -> bool {
        self.effect_strength > 0.0
            && self.frost > 0.0
            && (self.blur > 0.0 || self.roughness > 0.0 || self.edge_blur > 0.0)
    }
    pub fn edge_radius(self, width: u32, height: u32) -> u32 {
        if self.magnification != 0.0 {
            return width.max(height);
        }
        (self
            .edge_width
            .max(self.rim_width + self.rim_softness * 3.0)
            .max(self.max_displacement * (1.0 + self.dispersion * self.dispersion_base))
            + 4.0)
            .ceil() as u32
    }
    /// Blurred mips glass_shade can sample, of `available` above mip 0. Mirrors
    /// glass_common.wgsl: radius = blur + roughness term (depth >= 32) + edge
    /// blur, capped at 64; lod = clamp(log2(max(radius, 2)), 1, available).
    /// Trilinear filtering also reads the level above floor(lod).
    pub fn blur_levels(self, available: usize) -> usize {
        let corrected = self.roughness * (self.ior * 2.0 - 2.0).clamp(0.0, 1.0);
        let radius = (self.blur.max(0.0) + corrected * corrected * 1600.0 / 32.0 + self.edge_blur.max(0.0))
            .min(64.0);
        let lod = radius.max(2.0).log2().clamp(1.0, available.max(1) as f32);
        (lod.floor() as usize + 1).min(available)
    }
    /// Full-res pixels around covered glass that the blur pyramid must hold:
    /// the clamped refraction offset with dispersion, plus each level's
    /// bilinear and 3x3 Gaussian footprint (under 2^(levels+2) pixels).
    pub fn blur_reach(self, levels: usize) -> u32 {
        let displacement =
            self.max_displacement.max(0.0) * (1.0 + (self.dispersion * self.dispersion_base).abs());
        displacement.ceil() as u32 + 6 + (4u32 << levels.min(16))
    }
    pub fn uniform(self, sky: [f32; 3]) -> Uniform {
        Uniform {
            optical: [self.ior, self.thickness, self.distortion, self.blur],
            color: [self.tint[0], self.tint[1], self.tint[2], self.tint_strength],
            effects: [
                self.dispersion,
                self.roughness,
                self.reflection,
                self.style as u32 as f32,
            ],
            environment: [sky[0], sky[1], sky[2], self.foreground_guard],
            appearance: [
                self.frost,
                self.opacity,
                self.effect_strength,
                self.magnification,
            ],
            edge: [
                self.edge_width,
                self.edge_strength,
                self.edge_profile as u32 as f32,
                self.max_displacement,
            ],
            rim: [
                self.rim_strength,
                self.rim_width,
                self.rim_softness,
                self.rim_angle.to_radians(),
            ],
            detail: [
                self.edge_blur,
                self.edge_blur_mode,
                self.waviness,
                self.animation_speed,
            ],
            highlights: [
                self.rim_saturation,
                self.rim_type,
                self.dispersion_base,
                self.background_recovery,
            ],
        }
    }
}

static SETTINGS: RwLock<Settings> = RwLock::new(Settings::preset(Style::Liquid));
pub fn settings() -> Settings {
    *SETTINGS.read().unwrap_or_else(|e| e.into_inner())
}
pub fn set_style(style: Style) {
    *SETTINGS.write().unwrap_or_else(|e| e.into_inner()) = Settings::preset(style);
}
pub fn value(name: &str) -> f32 {
    settings().value(name)
}
impl Settings {
    pub fn value(self, name: &str) -> f32 {
        let s = self;
        match name {
            "r_glassBlur" => s.blur,
            "r_glassDistortion" => s.distortion,
            "r_glassDispersion" => s.dispersion,
            "r_glassRoughness" => s.roughness,
            "r_glassThickness" => s.thickness,
            "r_glassIOR" => s.ior,
            "r_glassTintStrength" => s.tint_strength,
            "r_glassReflection" => s.reflection,
            "r_glassForegroundGuard" => s.foreground_guard,
            "r_glassFrost" => s.frost,
            "r_glassOpacity" => s.opacity,
            "r_glassEffectStrength" => s.effect_strength,
            "r_glassEdgeWidth" => s.edge_width,
            "r_glassEdgeStrength" => s.edge_strength,
            "r_glassMaxDisplacement" => s.max_displacement,
            "r_glassMagnification" => s.magnification,
            "r_glassRimStrength" => s.rim_strength,
            "r_glassRimWidth" => s.rim_width,
            "r_glassRimSoftness" => s.rim_softness,
            "r_glassRimAngle" => s.rim_angle,
            "r_glassEdgeBlur" => s.edge_blur,
            "r_glassEdgeBlurMode" => s.edge_blur_mode,
            "r_glassWaviness" => s.waviness,
            "r_glassAnimationSpeed" => s.animation_speed,
            "r_glassRimSaturation" => s.rim_saturation,
            "r_glassRimType" => s.rim_type,
            "r_glassDispersionBase" => s.dispersion_base,
            "r_glassBackgroundRecovery" => s.background_recovery,
            _ => unreachable!("unknown glass control"),
        }
    }
}
pub fn set_value(name: &str, value: &str) -> Result<(), String> {
    if name == "r_glassEdgeProfile" {
        let profile = EdgeProfile::parse(value)
            .ok_or_else(|| "r_glassEdgeProfile: convex, squircle, concave, or lip".to_owned())?;
        SETTINGS
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .edge_profile = profile;
        return Ok(());
    }
    let v = value
        .parse::<f32>()
        .map_err(|_| format!("{name}: expected a number"))?;
    let (lo, hi) = match name {
        "r_glassBlur" => (0.0, 64.0),
        "r_glassForegroundGuard" => (0.0, 1.0),
        "r_glassDistortion" => (0.0, 8.0),
        "r_glassDispersion" => (0.0, 0.25),
        "r_glassThickness" => (0.1, 128.0),
        "r_glassIOR" => (1.0, 2.5),
        "r_glassRoughness" | "r_glassTintStrength" | "r_glassReflection" => (0.0, 1.0),
        "r_glassFrost" => (0.0, 1.0),
        "r_glassOpacity" => (0.0, 1.0),
        "r_glassEffectStrength" => (0.0, 1.0),
        "r_glassEdgeWidth" => (0.0, 128.0),
        "r_glassEdgeStrength" => (0.0, 8.0),
        "r_glassMaxDisplacement" => (0.0, 128.0),
        "r_glassMagnification" => (-1.0, 1.0),
        "r_glassRimStrength" => (0.0, 2.0),
        "r_glassRimWidth" => (0.0, 32.0),
        "r_glassRimSoftness" => (0.1, 32.0),
        "r_glassRimAngle" => (0.0, 360.0),
        "r_glassEdgeBlur" => (0.0, 64.0),
        "r_glassEdgeBlurMode" => (0.0, 1.0),
        "r_glassWaviness" => (0.0, 4.0),
        "r_glassAnimationSpeed" => (0.0, 4.0),
        "r_glassRimSaturation" => (0.0, 4.0),
        "r_glassRimType" => (0.0, 1.0),
        "r_glassDispersionBase" => (0.0, 4.0),
        "r_glassBackgroundRecovery" => (0.0, 1.0),
        _ => return Err(format!("unknown glass control {name}")),
    };
    if !v.is_finite() || !(lo..=hi).contains(&v) {
        return Err(format!("{name}: expected {lo}..{hi}"));
    }
    let mut s = SETTINGS.write().unwrap_or_else(|e| e.into_inner());
    match name {
        "r_glassBlur" => s.blur = v,
        "r_glassDistortion" => s.distortion = v,
        "r_glassDispersion" => s.dispersion = v,
        "r_glassRoughness" => s.roughness = v,
        "r_glassThickness" => s.thickness = v,
        "r_glassIOR" => s.ior = v,
        "r_glassTintStrength" => s.tint_strength = v,
        "r_glassReflection" => s.reflection = v,
        "r_glassForegroundGuard" => s.foreground_guard = v,
        "r_glassFrost" => s.frost = v,
        "r_glassOpacity" => s.opacity = v,
        "r_glassEffectStrength" => s.effect_strength = v,
        "r_glassEdgeWidth" => s.edge_width = v,
        "r_glassEdgeStrength" => s.edge_strength = v,
        "r_glassMaxDisplacement" => s.max_displacement = v,
        "r_glassMagnification" => s.magnification = v,
        "r_glassRimStrength" => s.rim_strength = v,
        "r_glassRimWidth" => s.rim_width = v,
        "r_glassRimSoftness" => s.rim_softness = v,
        "r_glassRimAngle" => s.rim_angle = v,
        "r_glassEdgeBlur" => s.edge_blur = v,
        "r_glassEdgeBlurMode" => s.edge_blur_mode = v,
        "r_glassWaviness" => s.waviness = v,
        "r_glassAnimationSpeed" => s.animation_speed = v,
        "r_glassRimSaturation" => s.rim_saturation = v,
        "r_glassRimType" => s.rim_type = v,
        "r_glassDispersionBase" => s.dispersion_base = v,
        "r_glassBackgroundRecovery" => s.background_recovery = v,
        _ => unreachable!(),
    }
    Ok(())
}
pub fn set_tint(value: &str) -> Result<(), String> {
    let channels: Vec<f32> = value
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|_| "r_glassTint: expected three numbers in 0..1".to_owned())?;
    if channels.len() != 3
        || channels
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
    {
        return Err("r_glassTint: expected three numbers in 0..1".to_owned());
    }
    SETTINGS
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .tint
        .copy_from_slice(&channels);
    Ok(())
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Uniform {
    pub optical: [f32; 4],
    pub color: [f32; 4],
    pub effects: [f32; 4],
    pub environment: [f32; 4],
    pub appearance: [f32; 4],
    pub edge: [f32; 4],
    pub rim: [f32; 4],
    pub detail: [f32; 4],
    pub highlights: [f32; 4],
}

pub fn shader_bindings(group: u32) -> String {
    format!(
        "@group({group}) @binding(0) var glass_scene: texture_2d<f32>;\n\
        @group({group}) @binding(1) var glass_sampler: sampler;\n\
        @group({group}) @binding(2) var glass_depth: texture_2d<f32>;\n\
        @group({group}) @binding(4) var<uniform> glass_settings: GlassSettings;\n\
        @group({group}) @binding(5) var glass_edges: texture_2d<f32>;\n\
        @group({group}) @binding(6) var glass_mask: texture_2d<f32>;\n\
        @group({group}) @binding(7) var glass_background_scene: texture_2d<f32>;\n{}",
        include_str!("glass_common.wgsl")
    )
}
pub fn model_shader(base: &str, group: u32) -> String {
    format!(
        "{}\n{}\n{}\n{}",
        crate::weather::with_model_weather(base),
        shader_bindings(group),
        include_str!("predator_cloak.wgsl"),
        if base.contains("studio_lights:") {include_str!("studio_lighting.wgsl")}else{""},
    )
}
pub fn world_shader() -> String {
    format!(
        "{}\n{}",
        include_str!("glass_world.wgsl"),
        shader_bindings(2)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_levels_cover_every_lod_glass_shade_can_sample() {
        // (preset, available, expected): radius 0.72 -> lod 1; 4.12 -> 2.04;
        // Frosted 29.1 -> 4.86. Trilinear needs floor(lod) + 1.
        for (style, expected) in [(Style::Bevy, 2), (Style::Liquid, 3), (Style::Frosted, 5)] {
            assert_eq!(Settings::preset(style).blur_levels(6), expected, "{style:?}");
        }
        let mut wide = Settings::preset(Style::Frosted);
        wide.blur = 500.0;
        assert_eq!(wide.blur_levels(6), 6, "radius caps at 64 = lod 6, all levels");
        assert_eq!(wide.blur_levels(3), 3, "never more than the pyramid holds");
    }

    #[test]
    fn blur_reach_holds_dispersed_displacement_and_pyramid_footprint() {
        let liquid = Settings::preset(Style::Liquid);
        // 32 * (1 + 0.06) = 33.92 -> 34, + 6 margin, + 2^(3 + 2).
        assert_eq!(liquid.blur_reach(3), 34 + 6 + 32);
        assert!(edges::edge_pad(liquid.edge_radius(2560, 1440)) >= 2 * liquid.edge_radius(2560, 1440));
    }
}

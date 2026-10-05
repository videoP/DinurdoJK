//! Linear-light split toning baked into the existing color LUT.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplitToningSettings {
    pub enabled: bool,
    pub strength: f32,
    pub shadow_hue: f32,
    pub shadow_saturation: f32,
    pub highlight_hue: f32,
    pub highlight_saturation: f32,
    pub balance: f32,
}

impl Default for SplitToningSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            strength: 0.5,
            shadow_hue: 210.0,
            shadow_saturation: 0.3,
            highlight_hue: 42.0,
            highlight_saturation: 0.25,
            balance: 0.0,
        }
    }
}

impl SplitToningSettings {
    pub fn sanitize(self) -> Self {
        let defaults = Self::default();
        let finite = |v: f32, fallback: f32, min: f32, max: f32| {
            if v.is_finite() {
                v.clamp(min, max)
            } else {
                fallback
            }
        };
        Self {
            enabled: self.enabled,
            strength: finite(self.strength, defaults.strength, 0.0, 1.0),
            shadow_hue: finite(self.shadow_hue, defaults.shadow_hue, 0.0, 360.0),
            shadow_saturation: finite(self.shadow_saturation, defaults.shadow_saturation, 0.0, 1.0),
            highlight_hue: finite(self.highlight_hue, defaults.highlight_hue, 0.0, 360.0),
            highlight_saturation: finite(
                self.highlight_saturation,
                defaults.highlight_saturation,
                0.0,
                1.0,
            ),
            balance: finite(self.balance, defaults.balance, -1.0, 1.0),
        }
    }

    pub fn active(self) -> bool {
        self.enabled
            && self.strength > 0.001
            && (self.shadow_saturation > 0.0 || self.highlight_saturation > 0.0)
    }

    /// CPU-only grading in linear light. Range masks use perceived brightness
    /// of the scene before output gamma and before any film/external LUT.
    pub fn apply_with_reference(self, rgb: [f64; 3], reference: [f64; 3]) -> [f64; 3] {
        if !self.active() {
            return rgb;
        }
        let brightness = linear_to_srgb(luminance(reference).clamp(0.0, 1.0));
        let crossover = 0.5 + self.balance as f64 * 0.3;
        // Overlapping ranges avoid a dead band around middle grey. Balance
        // moves the crossover; positive balance gives more pixels shadow tint.
        let t = ((brightness - (crossover - 0.25)) / 0.5).clamp(0.0, 1.0);
        let highlight_weight = t * t * (3.0 - 2.0 * t);
        let shadow_weight = 1.0 - highlight_weight;
        let shadow = hue_chroma(self.shadow_hue as f64);
        let highlight = hue_chroma(self.highlight_hue as f64);
        let luma = luminance(rgb).clamp(0.0, 1.0);
        // Scale tint with available light rather than adding colored fog to
        // black. Pure black and white remain unchanged.
        let headroom = 2.0 * luma.min(1.0 - luma);
        let delta: [f64; 3] = std::array::from_fn(|c| {
            headroom
                * self.strength as f64
                * (shadow[c] * self.shadow_saturation as f64 * shadow_weight
                    + highlight[c] * self.highlight_saturation as f64 * highlight_weight)
        });
        // Fit the whole chroma vector into gamut together. Per-channel clipping
        // would change luminance and introduce a brightness shift at high tints.
        let mut scale: f64 = 1.0;
        for c in 0..3 {
            if delta[c] > 0.0 {
                scale = scale.min((1.0 - rgb[c]) / delta[c]);
            } else if delta[c] < 0.0 {
                scale = scale.min(rgb[c] / -delta[c]);
            }
        }
        std::array::from_fn(|c| (rgb[c] + delta[c] * scale.max(0.0)).clamp(0.0, 1.0))
    }

    #[cfg(test)]
    fn apply(self, rgb: [f64; 3]) -> [f64; 3] {
        self.apply_with_reference(rgb, rgb)
    }
}

fn luminance(rgb: [f64; 3]) -> f64 {
    rgb[0] * 0.2126 + rgb[1] * 0.7152 + rgb[2] * 0.0722
}

fn linear_to_srgb(v: f64) -> f64 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn srgb_to_linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn hue_chroma(hue: f64) -> [f64; 3] {
    let h = hue.rem_euclid(360.0) / 60.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    let rgb = match h as u32 {
        0 => [1.0, x, 0.0],
        1 => [x, 1.0, 0.0],
        2 => [0.0, 1.0, x],
        3 => [0.0, x, 1.0],
        4 => [x, 0.0, 1.0],
        _ => [1.0, 0.0, x],
    }
    .map(srgb_to_linear);
    let luma = luminance(rgb);
    rgb.map(|v| v - luma)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_zero_strength_and_zero_saturation_are_identity() {
        let rgb = [0.12, 0.4, 0.85];
        for settings in [
            SplitToningSettings::default(),
            SplitToningSettings {
                enabled: true,
                strength: 0.0,
                ..Default::default()
            },
            SplitToningSettings {
                enabled: true,
                shadow_saturation: 0.0,
                highlight_saturation: 0.0,
                ..Default::default()
            },
        ] {
            assert_eq!(settings.apply(rgb), rgb);
        }
    }
    #[test]
    fn shadows_and_highlights_get_different_hues() {
        let settings = SplitToningSettings {
            enabled: true,
            strength: 0.5,
            shadow_hue: 240.0,
            shadow_saturation: 1.0,
            highlight_hue: 0.0,
            highlight_saturation: 1.0,
            ..Default::default()
        };
        let shadow = settings.apply([0.1; 3]);
        let highlight = settings.apply([0.9; 3]);
        assert!(shadow[2] > shadow[0]);
        assert!(highlight[0] > highlight[2]);
    }
    #[test]
    fn tint_preserves_luma_including_gamut_limits() {
        let settings = SplitToningSettings {
            enabled: true,
            strength: 1.0,
            shadow_saturation: 1.0,
            highlight_saturation: 1.0,
            ..Default::default()
        };
        for rgb in [
            [0.001; 3],
            [0.25; 3],
            [0.75; 3],
            [0.99; 3],
            [0.05, 0.9, 0.3],
            [0.0, 0.1, 1.0],
        ] {
            let out = settings.apply(rgb);
            let luma = out[0] * 0.2126 + out[1] * 0.7152 + out[2] * 0.0722;
            assert!((luma - luminance(rgb)).abs() < 1e-10);
        }
    }
    #[test]
    fn black_and_white_stay_neutral() {
        let settings = SplitToningSettings {
            enabled: true,
            strength: 1.0,
            shadow_saturation: 1.0,
            highlight_saturation: 1.0,
            ..Default::default()
        };
        assert_eq!(settings.apply([0.0; 3]), [0.0; 3]);
        assert_eq!(settings.apply([1.0; 3]), [1.0; 3]);
    }

    #[test]
    fn visibly_bright_linear_grey_receives_highlight_tint() {
        let settings = SplitToningSettings {
            enabled: true,
            ..Default::default()
        };
        // 65% on an sRGB display is only 38% in linear light. The former
        // library masks erroneously put this pixel in the shadow range.
        let grey = srgb_to_linear(0.65);
        let output = settings.apply([grey; 3]);
        assert!(
            output[0] > output[2],
            "visible highlight must be warm: {output:?}"
        );
        let dark = settings.apply([srgb_to_linear(0.25); 3]);
        assert!(dark[2] > dark[0], "shadow must be cool: {dark:?}");
    }

    #[test]
    fn balance_moves_ranges_and_masks_follow_the_original_scene() {
        let settings = SplitToningSettings {
            enabled: true,
            ..Default::default()
        };
        let grey = [srgb_to_linear(0.5); 3];
        let cool = SplitToningSettings {
            balance: 1.0,
            ..settings
        }
        .apply(grey);
        let warm = SplitToningSettings {
            balance: -1.0,
            ..settings
        }
        .apply(grey);
        assert!(cool[2] > cool[0]);
        assert!(warm[0] > warm[2]);
        // An external look that brightens a shadow must not reclassify it.
        let scene_shadow = [srgb_to_linear(0.25); 3];
        let output = settings.apply_with_reference([0.4; 3], scene_shadow);
        assert!(output[2] > output[0]);
    }

    #[test]
    fn invalid_parameters_are_finite_and_bounded() {
        let settings = SplitToningSettings {
            enabled: true,
            strength: f32::NAN,
            shadow_hue: f32::INFINITY,
            shadow_saturation: -1.0,
            highlight_hue: -30.0,
            highlight_saturation: 3.0,
            balance: f32::NEG_INFINITY,
        }
        .sanitize();
        assert_eq!(settings.strength, 0.5);
        assert_eq!(settings.shadow_hue, 210.0);
        assert_eq!(settings.shadow_saturation, 0.0);
        assert_eq!(settings.highlight_hue, 0.0);
        assert_eq!(settings.highlight_saturation, 1.0);
        assert_eq!(settings.balance, 0.0);
        assert!(settings
            .apply([0.4; 3])
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v)));
    }
}

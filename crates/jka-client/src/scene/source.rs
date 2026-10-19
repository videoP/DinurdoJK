//! Source.
use crate::scene::{BTreeMap, DirectionalSun, Shader, ShaderSun};

pub(in crate::scene) fn render_sun(sun: ShaderSun) -> DirectionalSun {
    DirectionalSun::from_q3_angles(sun.color, sun.intensity, sun.azimuth, sun.elevation)
}

pub(in crate::scene) fn authored_sun<'a>(
    shader_names: impl Iterator<Item = &'a str>,
    library: &BTreeMap<String, Shader>,
    warnings: &mut Vec<String>,
) -> Option<DirectionalSun> {
    let mut strongest: Option<(&str, ShaderSun)> = None;
    let mut count = 0usize;
    for name in shader_names {
        let Some(shader) = library.get(name) else {
            continue;
        };
        if !shader.sky {
            continue;
        }
        for &sun in &shader.suns {
            count += 1;
            if strongest.is_none_or(|(_, current)| sun.intensity > current.intensity) {
                strongest = Some((name, sun));
            }
        }
    }
    let (shader_name, sun) = strongest?;
    if count > 1 {
        warnings.push(format!(
            "{count} authored sky suns are referenced; using strongest sun from {shader_name} ({:.0} intensity)",
            sun.intensity
        ));
    }
    Some(render_sun(sun))
}

pub(in crate::scene) fn validate_map_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.split('/').any(|segment| {
            segment.is_empty()
                || !segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        })
    {
        return Err("Use a map name such as mp/ffa3, optionally followed by .bsp or .map.".into());
    }
    Ok(())
}

pub(in crate::scene) fn map_asset_name(name: &str, extension: &str) -> Result<String, String> {
    validate_map_name(name)?;
    Ok(format!("maps/{name}.{extension}"))
}

/// Use authored compiler settings when present and standard JK defaults otherwise.
pub(in crate::scene) fn baked_sun_encoding(
    bsp: &jka_assets::bsp::Bsp,
    shader_names: &[String],
    library: &BTreeMap<String, Shader>,
) -> Option<crate::scene::BakedSunEncoding> {
    let command = bsp.entities.iter()
        .find(|entity| entity.get(b"classname") == Some(b"worldspawn"))
        .and_then(|world| world.get(b"_q3map2_cmdline"))
        .and_then(|bytes| std::str::from_utf8(bytes).ok());
    let sun = shader_names.iter().filter_map(|name| library.get(name)).filter(|s| s.sky)
        .flat_map(|s| s.suns.iter()).max_by(|a, b| a.intensity.total_cmp(&b.intensity))?;
    baked_sun_compiler_settings(command, sun.deviance, sun.samples)
}

fn baked_sun_compiler_settings(command: Option<&str>, deviance: f32, samples: u32) -> Option<crate::scene::BakedSunEncoding> {
    let command = command.unwrap_or("");
    let words: Vec<_> = command.split_whitespace().collect();
    let parameter = |name: &str, default: f32| -> Option<f32> {
        match words.iter().rposition(|word| *word == name) {
            Some(index) => words.get(index + 1)?.trim_end_matches(';').parse::<f32>().ok().filter(|v| v.is_finite()),
            None => Some(default),
        }
    };
    let exposure = parameter("-exposure", 0.0)?;
    // Zero exposure is the standard linear/clamped encoding, not a missing profile.
    if exposure < 0.0 { return None; }
    let gamma = parameter("-gamma", 1.0)?;
    let compensate = parameter("-compensate", 1.0)?;
    let brightness = parameter("-brightness", 1.0)?;
    if gamma <= 0.0 || compensate <= 0.0 || brightness <= 0.0 { return None; }
    Some(crate::scene::BakedSunEncoding {
        exposure, gamma, compensate, brightness,
        deviance, samples: samples.clamp(1, 64),
    })
}

#[cfg(test)]
mod baked_sun_tests {
    use super::baked_sun_compiler_settings;

    #[test]
    fn baked_sun_missing_compiler_metadata_uses_jk_defaults() {
        for command in [None, Some("q3map2 -light map.bsp")] {
            let settings = baked_sun_compiler_settings(command, 0.0, 1).unwrap();
            assert_eq!([settings.exposure, settings.gamma, settings.compensate, settings.brightness], [0.0, 1.0, 1.0, 1.0]);
        }
    }

    #[test]
    fn baked_sun_explicit_compiler_settings_override_defaults() {
        let settings = baked_sun_compiler_settings(Some("-exposure 260 -gamma 2.2 -compensate 2 -brightness 1.5"), 2.0, 12).unwrap();
        assert_eq!([settings.exposure, settings.gamma, settings.compensate, settings.brightness], [260.0, 2.2, 2.0, 1.5]);
        assert_eq!(settings.samples, 12);
        assert!(baked_sun_compiler_settings(Some("-gamma 0"), 0.0, 1).is_none());
        assert!(baked_sun_compiler_settings(Some("-exposure -1"), 0.0, 1).is_none());
    }
}

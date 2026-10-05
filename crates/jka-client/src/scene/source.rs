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

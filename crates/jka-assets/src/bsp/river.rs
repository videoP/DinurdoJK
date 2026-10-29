//! Client-side map metadata for the Unreal river port.
//!
//! JAPRO supplies the entity vocabulary, not the solver. In particular its
//! Chezy `friction` must not be passed unchanged to Unreal's Manning friction.
//! All positions, bounds and velocities here remain in Z-up JKA map units.

use super::{Bsp, Entity, Model};

#[derive(Clone, Debug, PartialEq)]
pub struct Volume {
    pub model: usize,
    pub origin: [f32; 3],
    pub mins: [f32; 3],
    pub maxs: [f32; 3],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub cell_size: f32,
    /// Authored JAPRO coefficient; deliberately not named Manning friction.
    pub japro_friction: f32,
    pub capture_headroom: f32,
    pub base_shaders: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Inlet {
    DischargeCfs(f32),
    Velocity(f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outlet {
    Drain { rate: f32 },
    Open,
    Overfall,
    Normal,
    Fixed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub entity_index: usize,
    pub volume: Volume,
    pub inlet: Inlet,
    pub angle_degrees: f32,
    pub surface_height: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Sink {
    pub entity_index: usize,
    pub volume: Volume,
    pub outlet: Outlet,
    pub angle_degrees: f32,
    pub surface_height: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct River {
    pub entity_index: usize,
    pub name: String,
    pub volume: Volume,
    pub settings: Settings,
    pub sources: Vec<Source>,
    pub sinks: Vec<Sink>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub entity_index: usize,
    pub message: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rivers {
    pub rivers: Vec<River>,
    pub diagnostics: Vec<Diagnostic>,
}

fn text<'a>(entity: &'a Entity, key: &str) -> Result<Option<&'a str>, String> {
    entity
        .properties
        .iter()
        .rev()
        .find(|(name, _)| name.eq_ignore_ascii_case(key.as_bytes()))
        .map(|(_, value)| std::str::from_utf8(value).map_err(|_| format!("{key} is not UTF-8")))
        .transpose()
}

fn required<'a>(entity: &'a Entity, key: &str) -> Result<&'a str, String> {
    text(entity, key)?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("missing {key}"))
}

fn number(entity: &Entity, key: &str, default: f32) -> Result<f32, String> {
    let Some(value) = text(entity, key)? else {
        return Ok(default);
    };
    value
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("invalid finite {key}: {value:?}"))
}

fn bounded(entity: &Entity, key: &str, default: f32, min: f32, max: f32) -> Result<f32, String> {
    let value = number(entity, key, default)?;
    if value < min || value > max {
        return Err(format!("{key} outside {min}..={max}"));
    }
    Ok(value)
}

fn vector(entity: &Entity, key: &str) -> Result<[f32; 3], String> {
    let Some(value) = text(entity, key)? else {
        return Ok([0.0; 3]);
    };
    let parts: Vec<_> = value.split_ascii_whitespace().collect();
    if parts.len() != 3 {
        return Err(format!("{key} needs three coordinates"));
    }
    let mut result = [0.0; 3];
    for (out, part) in result.iter_mut().zip(parts) {
        *out = part
            .parse::<f32>()
            .ok()
            .filter(|value| value.is_finite())
            .ok_or_else(|| format!("invalid finite {key}: {value:?}"))?;
    }
    Ok(result)
}

fn volume(entity: &Entity, models: &[Model]) -> Result<Volume, String> {
    let model = required(entity, "model")?
        .strip_prefix('*')
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|&index| index > 0 && index < models.len())
        .ok_or_else(|| "river volume requires a valid inline brush model".to_owned())?;
    // Do not silently treat a rotated brush's unrotated box as its domain.
    if vector(entity, "angles")? != [0.0; 3] || number(entity, "angle", 0.0)? != 0.0 {
        // Source/sink `angle` is a flow heading, not a brush rotation.
        let classname = required(entity, "classname")?;
        if vector(entity, "angles")? != [0.0; 3]
            || classname.eq_ignore_ascii_case("misc_sailing_river")
        {
            return Err("rotated river volumes are not supported by the map adapter yet".into());
        }
    }
    let origin = vector(entity, "origin")?;
    let mins = std::array::from_fn(|axis| models[model].mins[axis] + origin[axis]);
    let maxs = std::array::from_fn(|axis| models[model].maxs[axis] + origin[axis]);
    if (0..3)
        .any(|axis| !mins[axis].is_finite() || !maxs[axis].is_finite() || maxs[axis] <= mins[axis])
    {
        return Err("river volume has empty or invalid bounds".into());
    }
    Ok(Volume {
        model,
        origin,
        mins,
        maxs,
    })
}

fn river(entity: &Entity, models: &[Model], entity_index: usize) -> Result<River, String> {
    let name = required(entity, "targetname")?.to_owned();
    let base_shaders = text(entity, "baseShader")?
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect();
    Ok(River {
        entity_index,
        name,
        volume: volume(entity, models)?,
        settings: Settings {
            cell_size: bounded(entity, "cellSize", 16.0, 4.0, 256.0)?,
            japro_friction: bounded(entity, "friction", 0.12, 0.001, 2.0)?,
            capture_headroom: bounded(entity, "captureHeadroom", 8.0, 0.0, 256.0)?,
            base_shaders,
        },
        sources: Vec::new(),
        sinks: Vec::new(),
    })
}

fn source(entity: &Entity, models: &[Model], entity_index: usize) -> Result<Source, String> {
    let volume = volume(entity, models)?;
    let kind = text(entity, "type")?.unwrap_or("discharge");
    let inlet = if kind.eq_ignore_ascii_case("discharge") {
        Inlet::DischargeCfs(bounded(entity, "discharge", 0.0, 0.0, f32::MAX)?)
    } else if kind.eq_ignore_ascii_case("velocity") {
        Inlet::Velocity(bounded(entity, "speed", 0.0, 0.0, f32::MAX)?)
    } else {
        return Err(format!("unsupported river source type {kind:?}"));
    };
    Ok(Source {
        entity_index,
        inlet,
        angle_degrees: number(entity, "angle", 0.0)?,
        surface_height: number(entity, "surfaceHeight", volume.maxs[2])?,
        volume,
    })
}

fn sink(entity: &Entity, models: &[Model], entity_index: usize) -> Result<Sink, String> {
    let volume = volume(entity, models)?;
    let kind = text(entity, "type")?.unwrap_or("open");
    let outlet = match kind.to_ascii_lowercase().as_str() {
        "drain" => Outlet::Drain {
            rate: bounded(entity, "drainrate", 0.2, 0.0, f32::MAX)?,
        },
        "open" => Outlet::Open,
        "overfall" => Outlet::Overfall,
        "normal" => Outlet::Normal,
        "fixed" => Outlet::Fixed,
        _ => return Err(format!("unsupported river sink type {kind:?}")),
    };
    Ok(Sink {
        entity_index,
        outlet,
        angle_degrees: number(entity, "angle", 0.0)?,
        surface_height: number(entity, "surfaceHeight", volume.maxs[2])?,
        volume,
    })
}

/// Read map-local entities only. No server state, simulation, bake loading, or
/// fallback-surface hiding happens here. Call again after applying a .ent edit.
pub fn parse(entities: &[Entity], models: &[Model]) -> Rivers {
    let mut result = Rivers::default();
    for (entity_index, entity) in entities.iter().enumerate() {
        if !matches!(text(entity, "classname"), Ok(Some(name)) if name.eq_ignore_ascii_case("misc_sailing_river"))
        {
            continue;
        }
        let parsed = river(entity, models, entity_index).and_then(|river| {
            if result
                .rivers
                .iter()
                .any(|other| other.name.eq_ignore_ascii_case(&river.name))
            {
                Err(format!("duplicate river targetname {:?}", river.name))
            } else {
                Ok(river)
            }
        });
        match parsed {
            Ok(river) => result.rivers.push(river),
            Err(message) => result.diagnostics.push(Diagnostic {
                entity_index,
                message,
            }),
        }
    }
    // Sources and sinks often precede the river entity in a compiled BSP.
    for (entity_index, entity) in entities.iter().enumerate() {
        let Ok(Some(classname)) = text(entity, "classname") else {
            continue;
        };
        let is_source = classname.eq_ignore_ascii_case("misc_sailing_river_source");
        if !is_source && !classname.eq_ignore_ascii_case("misc_sailing_river_sink") {
            continue;
        }
        let added = required(entity, "target").and_then(|target| {
            let river = result
                .rivers
                .iter_mut()
                .find(|river| river.name.eq_ignore_ascii_case(target))
                .ok_or_else(|| format!("river target {target:?} does not resolve"))?;
            if is_source {
                river.sources.push(source(entity, models, entity_index)?);
            } else {
                river.sinks.push(sink(entity, models, entity_index)?);
            }
            Ok(())
        });
        if let Err(message) = added {
            result.diagnostics.push(Diagnostic {
                entity_index,
                message,
            });
        }
    }
    result
}

impl Bsp {
    pub fn rivers(&self) -> Rivers {
        parse(&self.entities, &self.models)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(pairs: &[(&str, &str)]) -> Entity {
        Entity {
            properties: pairs
                .iter()
                .map(|(key, value)| (key.as_bytes().to_vec(), value.as_bytes().to_vec()))
                .collect(),
        }
    }

    fn models() -> Vec<Model> {
        (0..4)
            .map(|_| Model {
                mins: [-16.0, -32.0, -64.0],
                maxs: [16.0, 32.0, 64.0],
                surfaces: 0..0,
                brushes: 0..0,
            })
            .collect()
    }

    fn tsangpo_entities() -> Vec<Entity> {
        vec![
            entity(&[
                ("classname", "misc_sailing_river_sink"),
                ("target", "tsangpo"),
                ("type", "drain"),
                ("angle", "143"),
                ("drainrate", "0.2"),
                ("model", "*1"),
            ]),
            entity(&[
                ("classname", "misc_sailing_river_source"),
                ("target", "tsangpo"),
                ("type", "discharge"),
                ("discharge", "30000"),
                ("angle", "346"),
                ("model", "*2"),
            ]),
            entity(&[
                ("classname", "misc_sailing_river"),
                ("targetname", "tsangpo"),
                ("cellSize", "32"),
                ("friction", "0.03"),
                ("captureHeadroom", "32"),
                ("model", "*3"),
            ]),
        ]
    }

    #[test]
    fn tsangpo_contract_resolves_boundaries_before_river_without_changing_units() {
        let result = parse(&tsangpo_entities(), &models());
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);
        let river = &result.rivers[0];
        assert_eq!(river.volume.model, 3);
        assert_eq!(river.settings.cell_size, 32.0);
        assert_eq!(river.settings.japro_friction, 0.03);
        assert_eq!(river.sources[0].inlet, Inlet::DischargeCfs(30000.0));
        assert_eq!(river.sources[0].angle_degrees, 346.0);
        assert_eq!(river.sinks[0].outlet, Outlet::Drain { rate: 0.2 });
        assert_eq!(river.sinks[0].surface_height, 64.0);
    }

    #[test]
    fn malformed_or_unresolved_entities_produce_diagnostics_without_losing_valid_rivers() {
        let mut entities = tsangpo_entities();
        entities.push(entity(&[
            ("classname", "misc_sailing_river_source"),
            ("target", "absent"),
            ("model", "*1"),
        ]));
        entities.push(entity(&[
            ("classname", "misc_sailing_river"),
            ("targetname", "bad"),
            ("model", "*99"),
        ]));
        entities.push(entity(&[
            ("classname", "misc_sailing_river_source"),
            ("target", "tsangpo"),
            ("model", "*1"),
            ("discharge", "NaN"),
        ]));
        let result = parse(&entities, &models());
        assert_eq!(result.rivers.len(), 1);
        assert_eq!(result.rivers[0].sources.len(), 1);
        assert_eq!(result.diagnostics.len(), 3);
    }

    #[test]
    fn multiple_rivers_keep_boundaries_separate_and_preserve_exact_shader_names() {
        let mut entities = tsangpo_entities();
        entities.push(entity(&[
            ("classname", "misc_sailing_river"),
            ("targetname", "second"),
            ("model", "*3"),
            ("origin", "100 0 20"),
            ("baseShader", "textures/River/Water, textures/other"),
        ]));
        entities.push(entity(&[
            ("classname", "misc_sailing_river_source"),
            ("target", "second"),
            ("model", "*2"),
            ("type", "velocity"),
            ("speed", "120"),
        ]));
        let result = parse(&entities, &models());
        assert!(result.diagnostics.is_empty());
        assert_eq!(result.rivers[0].sources.len(), 1);
        assert_eq!(result.rivers[1].sources[0].inlet, Inlet::Velocity(120.0));
        assert_eq!(
            result.rivers[1].settings.base_shaders,
            ["textures/River/Water", "textures/other"]
        );
        assert_eq!(result.rivers[1].volume.mins, [84.0, -32.0, -44.0]);
    }

    #[test]
    fn duplicate_names_and_unknown_boundary_types_are_explicit() {
        let mut entities = tsangpo_entities();
        entities.push(entities[2].clone());
        entities.push(entity(&[
            ("classname", "misc_sailing_river_sink"),
            ("target", "tsangpo"),
            ("model", "*1"),
            ("type", "invented"),
        ]));
        let result = parse(&entities, &models());
        assert_eq!(result.rivers.len(), 1);
        assert_eq!(result.rivers[0].sinks.len(), 1);
        assert_eq!(result.diagnostics.len(), 2);
    }
}

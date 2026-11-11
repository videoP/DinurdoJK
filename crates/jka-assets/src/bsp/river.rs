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

/// Riverbed geometry: what players collide with, not what is textured.
/// Candidate faces are every side polygon of the world model's solid/terrain
/// brushes (exact brush-plane clipping, so caulk/nodraw faces count), solid
/// curved patches (eight segments per quadratic span, as the coastal
/// bathymetry samples them) and solid triangle soups. Liquid and clip-only
/// brushes are not bed. A face is kept only if it can be ground:
///  - its outward normal points up (a ceiling's underside never is), and
///  - for patches and triangle soups only, the point one unit in front of it
///    is in a playable BSP cluster (maps compiled without vis skip this).
///    Solid brush tops are kept even when buried in a ceiling or the sky:
///    the capture keeps the highest surface, as Unreal's depth render does,
///    so a wall running up into the ceiling is a full-height wall instead of
///    a hole. Roofs are still dropped by their solid's bottom height.
/// Sky flags are honoured when the BSP carries them (Tsangpo's does not).
/// Z-up JKA map units; returned triangles face up.
/// Up-facing collision surface with the bottom of its parent solid brush.
/// The brush support height distinguishes a box standing in the water from a
/// roof suspended above it. Mesh terrain (patches/triangle soups) has no
/// parent solid, so it is clipped to the requested water level instead.
#[derive(Clone, Copy, Debug)]
pub struct CollisionSurface {
    pub triangle: [[f32; 3]; 3],
    pub solid_bottom_z: Option<f32>,
}

/// Preserve the original unclassified triangle API for generic/map captures.
pub fn collision_triangles(bsp: &Bsp) -> Result<Vec<[[f32; 3]; 3]>, String> {
    Ok(collision_surfaces(bsp)?.into_iter().map(|s| s.triangle).collect())
}

/// Detailed developer-river collision, built once per map and reused by each
/// `riversource` placement without touching collision on the render loop.
pub fn collision_surfaces(bsp: &Bsp) -> Result<Vec<CollisionSurface>, String> {
    use super::{brush_side_polygon, patch_vertex, SurfaceKind, CONTENTS_SOLID, CONTENTS_TERRAIN, SURF_SKY};
    const LIQUIDS: u32 = super::bathymetry::CONTENTS_WATER | 0x0000_0002 | 0x0002_0000;
    const UP: f32 = 1e-3;
    let solid = |contents: u32| contents & (CONTENTS_SOLID | CONTENTS_TERRAIN) != 0 && contents & LIQUIDS == 0;
    let world = bsp.models.first().ok_or("BSP has no world model")?;
    let extent = world.mins.iter().chain(&world.maxs).map(|v| v.abs()).fold(1024.0, f32::max) * 2.0;
    let in_air = |points: &[[f32; 3]], normal: [f32; 3]| -> bool {
        let Some(vis) = bsp.visibility.as_ref() else { return true };
        let n = points.len() as f32;
        let probe = std::array::from_fn(|a| points.iter().map(|p| p[a]).sum::<f32>() / n + normal[a]);
        vis.cluster_at(probe).is_some()
    };
    let mut out = Vec::new();
    for id in world.brushes.clone() {
        let brush = bsp.brushes.get(id).ok_or("invalid world brush index")?;
        let contents = bsp.shaders.get(brush.shader).ok_or("invalid brush shader")?.contents;
        if !solid(contents) {
            continue;
        }
        let start = out.len();
        let mut bottom_z = f32::INFINITY;
        for side in brush.sides.clone() {
            let bside = bsp.brush_sides.get(side).ok_or("invalid brush side")?;
            let flags = bsp.shaders.get(bside.shader).map_or(0, |s| s.surface_flags);
            let plane = bsp.planes.get(bside.plane).ok_or("invalid brush plane")?;
            // All sides contribute to the brush's lowest occupied height, not
            // just its upward top. A floating roof has its bottom above water;
            // a standing box has its bottom below the water surface.
            let polygon = brush_side_polygon(bsp, brush, side, extent).map_err(|e| e.to_string())?;
            for point in &polygon {
                bottom_z = bottom_z.min(point[2]);
            }
            if plane.normal[2] <= UP || flags & SURF_SKY != 0 || polygon.len() < 3 {
                continue;
            }
            for k in 2..polygon.len() {
                out.push(CollisionSurface {
                    triangle: [polygon[0], polygon[k - 1], polygon[k]],
                    solid_bottom_z: None,
                });
            }
        }
        // Assign once after scanning every side. This also handles sloped and
        // non-axis-aligned brushes; no guessed brush height or threshold.
        if bottom_z.is_finite() {
            for surface in &mut out[start..] {
                surface.solid_bottom_z = Some(bottom_z);
            }
        }
    }
    for id in world.surfaces.clone() {
        let surface = bsp.surfaces.get(id).ok_or("invalid world surface index")?;
        let shader = bsp.shaders.get(surface.shader).ok_or("invalid surface shader")?;
        if !solid(shader.contents) || shader.surface_flags & SURF_SKY != 0 {
            continue;
        }
        let verts = bsp.vertices.get(surface.vertices.clone()).ok_or("invalid surface vertices")?;
        // Orient each triangle by its authored vertex normals, as bathymetry does.
        let mut push = |v: [super::Vertex; 3]| {
            let p = v.map(|v| v.position);
            let e1: [f32; 3] = std::array::from_fn(|a| p[1][a] - p[0][a]);
            let e2: [f32; 3] = std::array::from_fn(|a| p[2][a] - p[0][a]);
            let mut n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
            let authored: [f32; 3] = std::array::from_fn(|a| v.iter().map(|v| v.normal[a]).sum());
            if n[0] * authored[0] + n[1] * authored[1] + n[2] * authored[2] < 0.0 {
                n = n.map(|c| -c);
            }
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len < 1e-6 {
                return;
            }
            let n = n.map(|c| c / len);
            if n[2] > UP && in_air(&p, n) {
                out.push(CollisionSurface { triangle: p, solid_bottom_z: None });
            }
        };
        match surface.kind {
            SurfaceKind::Patch => {
                let [w, h] = surface.patch_size;
                if w < 3 || h < 3 || verts.len() < w * h {
                    continue;
                }
                for y in (0..h - 2).step_by(2) {
                    for x in (0..w - 2).step_by(2) {
                        for j in 0..8 {
                            for i in 0..8 {
                                let q = |u: usize, v: usize| patch_vertex(verts, w, x, y, u as f32 / 8.0, v as f32 / 8.0);
                                let (a, b, c, d) = (q(i, j), q(i + 1, j), q(i, j + 1), q(i + 1, j + 1));
                                push([a, c, b]);
                                push([b, c, d]);
                            }
                        }
                    }
                }
            }
            // Planar faces are brush sides, already handled above.
            SurfaceKind::Triangles => {
                let indices = bsp.indices.get(surface.indices.clone()).ok_or("invalid surface indices")?;
                for t in indices.chunks_exact(3) {
                    if let (Some(a), Some(b), Some(c)) =
                        (verts.get(t[0] as usize), verts.get(t[1] as usize), verts.get(t[2] as usize))
                    {
                        push([*a, *b, *c]);
                    }
                }
            }
            SurfaceKind::Planar | SurfaceKind::Flare => {}
        }
    }
    Ok(out)
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

//! Entities.
use crate::scene::{
    q3map_color_normalize, Q3MAP_FALLOFF_TOLERANCE, Q3MAP_LIGHTMAP_BYTE_SCALE, Q3MAP_LINEAR_SCALE,
    Q3MAP_POINT_SCALE,
};
use crate::scene::{
    render_position, AssetSearchPath, BTreeMap, BlendMode, Bsp, CullMode, DrawBatch, DrawClass,
    DynamicLight, DynamicLightFalloff, GpuVertex, MapBrushEntity, MapFxRunner, PipelineKey,
    TrainCorner, Vec3,
};

/// Renderer-space sky portal camera (cgame `CG_DrawSkyBoxPortal`): the sky view
/// sits at `origin` plus the player's offset from `orient_origin`, scaled by
/// `scale`. Without a `misc_skyportal_orient` the scale is zero, so the sky
/// camera stays fixed at `origin`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyPortal {
    pub origin: [f32; 3],
    pub orient_origin: [f32; 3],
    pub scale: f32,
}

impl SkyPortal {
    pub fn camera_position(&self, view: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|axis| {
            self.origin[axis] + (view[axis] - self.orient_origin[axis]) * self.scale
        })
    }
}

pub(in crate::scene) fn bsp_sky_portal(bsp: &Bsp) -> Option<SkyPortal> {
    let mut portal: Option<[f32; 3]> = None;
    let mut orient: Option<([f32; 3], f32)> = None;
    for entity in &bsp.entities {
        let Some(classname) = entity
            .get(b"classname")
            .and_then(|value| std::str::from_utf8(value).ok())
        else {
            continue;
        };
        let origin = entity
            .get(b"origin")
            .and_then(|value| std::str::from_utf8(value).ok())
            .and_then(parse_light_triplet)
            .unwrap_or([0.0; 3]);
        if classname.eq_ignore_ascii_case("misc_skyportal") {
            portal.get_or_insert(origin);
        } else if classname.eq_ignore_ascii_case("misc_skyportal_orient") {
            let scale = entity
                .get(b"modelscale")
                .and_then(|value| std::str::from_utf8(value).ok())
                .and_then(parse_leading_f32)
                .unwrap_or(0.0);
            orient.get_or_insert((origin, scale));
        }
    }
    let origin = portal?;
    let (orient_origin, scale) = orient.unwrap_or(([0.0; 3], 0.0));
    Some(SkyPortal {
        origin: render_position(origin),
        orient_origin: render_position(orient_origin),
        scale,
    })
}

pub(in crate::scene) const DISTANCE_CULL_KEYS: [&str; 4] =
    ["distancecull", "_distancecull", "_farplanedist", "fogclip"];

pub(in crate::scene) fn parse_distance_cull(raw: &str) -> Option<f32> {
    // OpenJK uses sscanf(value, "%f", ...), so it accepts a valid leading
    // float even when mapping tools append a distance-measure suffix (for
    // example `24000r`). Rust's `str::parse::<f32>()` requires the entire
    // string to be numeric, so mirror sscanf's prefix behavior here.
    let value = parse_leading_f32(raw)?;
    (value.is_finite() && value > 1.0).then_some(value)
}

pub(in crate::scene) fn parse_leading_f32(raw: &str) -> Option<f32> {
    let text = raw.trim_start();
    if text.is_empty() {
        return None;
    }
    for end in (1..=text.len()).rev() {
        if !text.is_char_boundary(end) {
            continue;
        }
        if let Ok(value) = text[..end].trim_end().parse::<f32>() {
            return Some(value);
        }
    }
    None
}

pub(in crate::scene) fn bsp_worldspawn_distance_cull(
    bsp: &Bsp,
    warnings: &mut Vec<String>,
) -> Option<f32> {
    let Some(world) = bsp.entities.iter().find(|entity| {
        entity.properties.iter().rev().any(|(key, value)| {
            key.eq_ignore_ascii_case(b"classname") && value.eq_ignore_ascii_case(b"worldspawn")
        })
    }) else {
        return None;
    };

    for wanted in DISTANCE_CULL_KEYS {
        if let Some((key, raw)) = world
            .properties
            .iter()
            .rev()
            .find(|(key, _)| key.eq_ignore_ascii_case(wanted.as_bytes()))
        {
            let text = String::from_utf8_lossy(raw);
            if let Some(value) = parse_distance_cull(&text) {
                return Some(value);
            }
            warnings.push(format!(
                "worldspawn {} value {:?} is not a usable positive distance; using the client default far plane",
                String::from_utf8_lossy(key),
                text
            ));
            return None;
        }
    }
    None
}

pub(in crate::scene) fn map_worldspawn_distance_cull(
    world: &jka_assets::map::MapEntity,
    warnings: &mut Vec<String>,
) -> Option<f32> {
    for wanted in DISTANCE_CULL_KEYS {
        if let Some((key, raw)) = world
            .properties
            .iter()
            .rev()
            .find(|(key, _)| key.eq_ignore_ascii_case(wanted))
        {
            if let Some(value) = parse_distance_cull(raw) {
                return Some(value);
            }
            warnings.push(format!(
                "worldspawn {key} value {raw:?} is not a usable positive distance; using the client default far plane"
            ));
            return None;
        }
    }
    None
}

pub(in crate::scene) fn parse_light_triplet(value: &str) -> Option<[f32; 3]> {
    let values: Vec<f32> = value
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    if values.len() != 3 || !values.iter().all(|value| value.is_finite()) {
        return None;
    }
    values.try_into().ok()
}

pub(in crate::scene) fn dynamic_light_from_values(
    classname: &str,
    origin: Option<&str>,
    color: Option<&str>,
    brightness: Option<&str>,
) -> Option<DynamicLight> {
    if classname != "light" && classname != "lightJunior" {
        return None;
    }
    let origin = parse_light_triplet(origin?)?;
    let color = color
        .and_then(parse_light_triplet)
        .unwrap_or([1.0, 1.0, 1.0])
        .map(|value| value.max(0.0));
    let brightness = brightness
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .unwrap_or(300.0);
    // Radiant's `light` key is an authored brightness rather than a strict
    // physical radius. Using it as the enhancement cutoff keeps the modern
    // dynamic contribution local and predictable without changing the baked
    // lightmap that remains the primary lighting source.
    let radius = brightness.clamp(32.0, 4096.0);
    let intensity = (brightness / 300.0).clamp(0.05, 8.0);
    Some(DynamicLight {
        position: render_position(origin),
        color,
        radius,
        intensity,
        falloff: DynamicLightFalloff::Smooth,
        // Preserve the existing compiled-BSP enhancement path exactly. Source-map
        // lightJunior filtering is handled by map_dynamic_light below.
        surface_lighting: true,
        emitter_normal: [0.0; 3],
        emitter_two_sided: false,
        angle_attenuation: true,
        angle_scale: 0.0,
        extra_distance: 0.0,
    })
}

pub(crate) fn append_authored_ocean_planes(
    oceans: &[crate::ocean::authoring::AuthoredOcean],
    vertices: &mut Vec<GpuVertex>,
    batches: &mut Vec<DrawBatch>,
    pvs_batches: &mut Vec<DrawBatch>,
) {
    let Some(template) = batches
        .iter()
        .find(|b| b.water_primary)
        .or_else(|| batches.first())
        .cloned()
    else {
        return;
    };
    // The marker brush is a volume; its top becomes an independently rendered ocean.
    // Remove fully covered stock water stages to avoid drawing two surfaces there.
    let covered = |b: &DrawBatch| {
        b.water
            && oceans.iter().any(|o| {
                vertices[b.vertices.start as usize..b.vertices.end as usize]
                    .iter()
                    .all(|v| o.contains_render_point(v.position))
            })
    };
    batches.retain(|b| b.authored_ocean.is_none() && !covered(b));
    // AUTO 4 plans address FULL pieces by index, and this also runs after the
    // plan was built (networked oceans). Empty replaced pieces in place instead
    // of removing them so every plan index stays valid.
    for b in pvs_batches
        .iter_mut()
        .filter(|b| b.authored_ocean.is_some() || covered(&**b))
    {
        b.vertices = b.vertices.start..b.vertices.start;
        b.water = false;
        b.water_primary = false;
        b.authored_ocean = None;
    }
    for o in oceans {
        let first = vertices.len() as u32;
        let corners = [
            [o.mins[0], o.height, -o.maxs[1]],
            [o.maxs[0], o.height, -o.maxs[1]],
            [o.maxs[0], o.height, -o.mins[1]],
            [o.mins[0], o.height, -o.mins[1]],
        ];
        for i in [0, 2, 1, 0, 3, 2] {
            vertices.push(GpuVertex {
                position: corners[i],
                uv: [0.0; 2],
                lightmap_uv: [0.0; 2],
                normal: [0.0, 1.0, 0.0],
                color: [1.0; 4],
                alpha_cutoff: 1.0,
            });
        }
        let mut b = template.clone();
        b.vertices = first..vertices.len() as u32;
        b.water = true;
        b.water_primary = true;
        b.authored_ocean = Some(o.index);
        b.bsp_shader_index = u32::MAX;
        b.material_debug_index = u32::MAX;
        b.texture = None;
        b.texture_is_lightmap = false;
        b.texture_is_white = false;
        b.lightmap = None;
        b.modulate_lightmap = false;
        b.tc_mods.clear();
        b.color = [1.0; 4];
        b.alpha_cutoff = 0.0;
        b.pipeline = PipelineKey {
            class: DrawClass::Transparent,
            blend: BlendMode::Opaque,
            cull: CullMode::None,
            offset: false,
            depth_write: true,
            depth_equal: false,
        };
        b.pvs_signature.clear();
        b.area_signature = [0; 4];
        b.planar_reflection = false;
        b.planar_environment_candidate = false;
        batches.push(b.clone());
        pvs_batches.push(b);
    }
}

pub(in crate::scene) fn bsp_entity_value<'a>(
    entity: &'a jka_assets::bsp::Entity,
    key: &[u8],
) -> Option<&'a [u8]> {
    entity
        .properties
        .iter()
        .rev()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_slice())
}

pub(in crate::scene) fn parse_bsp_entity_triplet(value: &[u8]) -> Option<[f32; 3]> {
    let text = std::str::from_utf8(value).ok()?;
    let mut values = text.split_whitespace().map(str::parse::<f32>);
    let result = [
        values.next()?.ok()?,
        values.next()?.ok()?,
        values.next()?.ok()?,
    ];
    if values.next().is_some() || !result.iter().all(|value| value.is_finite()) {
        return None;
    }
    Some(result)
}

pub(in crate::scene) fn parse_bsp_entity_i32(value: Option<&[u8]>, default: i32) -> i32 {
    value
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.trim().parse::<i32>().ok())
        .unwrap_or(default)
}

pub(in crate::scene) fn parse_bsp_entity_f32(value: Option<&[u8]>, default: f32) -> f32 {
    value
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
        .unwrap_or(default)
}

pub(in crate::scene) fn vector_to_jka_angles(direction: [f32; 3]) -> [f32; 3] {
    let [x, y, z] = direction;
    if x == 0.0 && y == 0.0 {
        return [if z > 0.0 { -90.0 } else { 90.0 }, 0.0, 0.0];
    }
    let yaw = y.atan2(x).to_degrees().rem_euclid(360.0);
    let forward = (x * x + y * y).sqrt();
    let pitch = z.atan2(forward).to_degrees().rem_euclid(360.0);
    [-pitch, yaw, 0.0]
}

pub(in crate::scene) fn bsp_entity_angles(entity: &jka_assets::bsp::Entity) -> [f32; 3] {
    if let Some(angles) = bsp_entity_value(entity, b"angles").and_then(parse_bsp_entity_triplet) {
        return angles;
    }
    if let Some(angle) = bsp_entity_value(entity, b"angle")
        .and_then(|value| std::str::from_utf8(value).ok())
        .and_then(|value| value.trim().parse::<f32>().ok())
        .filter(|value| value.is_finite())
    {
        // id Tech's ANGLE_UP / ANGLE_DOWN shortcuts.
        return match angle as i32 {
            -1 => [-90.0, 0.0, 0.0],
            -2 => [90.0, 0.0, 0.0],
            _ => [0.0, angle, 0.0],
        };
    }
    [0.0; 3]
}

/// Think_SetupTrainTargets: walk from the entity named `target`, linking each
/// corner to the first `path_corner` its own `target` names. The walk ends at a
/// corner without a target/successor, or where it re-enters the route.
pub(in crate::scene) fn bsp_train_corners(bsp: &Bsp, target: &[u8]) -> Vec<TrainCorner> {
    let find = |name: &[u8], path_corner_only: bool| {
        bsp.entities.iter().position(|entity| {
            bsp_entity_value(entity, b"targetname") == Some(name)
                && (!path_corner_only
                    || bsp_entity_value(entity, b"classname") == Some(b"path_corner".as_slice()))
        })
    };
    let mut entity_of_corner = Vec::<usize>::new();
    let mut corners = Vec::<TrainCorner>::new();
    let mut current = find(target, false);
    while let Some(entity_index) = current {
        let entity = &bsp.entities[entity_index];
        let index = corners.len();
        entity_of_corner.push(entity_index);
        corners.push(TrainCorner {
            origin: bsp_entity_value(entity, b"origin")
                .and_then(parse_bsp_entity_triplet)
                .unwrap_or([0.0; 3]),
            speed: parse_bsp_entity_f32(bsp_entity_value(entity, b"speed"), 0.0),
            wait: parse_bsp_entity_f32(bsp_entity_value(entity, b"wait"), 0.0),
            next: None,
        });
        let next_entity = bsp_entity_value(entity, b"target").and_then(|name| find(name, true));
        match next_entity {
            Some(next_entity) => {
                if let Some(seen) = entity_of_corner
                    .iter()
                    .position(|&seen| seen == next_entity)
                {
                    corners[index].next = Some(seen);
                    break;
                }
                corners[index].next = Some(index + 1);
                current = Some(next_entity);
            }
            None => break,
        }
    }
    corners
}

pub(crate) fn bsp_brush_entities(bsp: &Bsp) -> Vec<MapBrushEntity> {
    bsp.entities
        .iter()
        .filter_map(|entity| {
            let spawn_vars = entity
                .properties
                .iter()
                .map(|(key, value)| {
                    (
                        String::from_utf8_lossy(key).into_owned(),
                        String::from_utf8_lossy(value).into_owned(),
                    )
                })
                .collect();
            // SV_SetBrushModel: `*N` is atoi(name + 1); model 0 is the world.
            let brush_model = bsp_entity_value(entity, b"model")
                .and_then(|name| std::str::from_utf8(name).ok())
                .and_then(|name| name.trim().strip_prefix('*')?.parse::<u32>().ok())
                .filter(|&model| model > 0);
            let Some(model) = brush_model else {
                // The point entities that carry a use from triggers to movers.
                let logic = bsp_entity_value(entity, b"classname").is_some_and(|class| {
                    [
                        &b"target_relay"[..],
                        b"target_delay",
                        b"target_activate",
                        b"target_deactivate",
                        b"target_counter",
                        b"trigger_always",
                    ]
                    .iter()
                    .any(|logic| class.eq_ignore_ascii_case(logic))
                });
                return logic.then_some(MapBrushEntity {
                    model: 0,
                    mins: [0.0; 3],
                    maxs: [0.0; 3],
                    train_corners: Vec::new(),
                    spawn_vars,
                });
            };
            let bounds = bsp.models.get(model as usize)?;
            let is_train = bsp_entity_value(entity, b"classname")
                .is_some_and(|class| class.eq_ignore_ascii_case(b"func_train"));
            let train_corners = if is_train {
                bsp_entity_value(entity, b"target")
                    .map(|target| bsp_train_corners(bsp, target))
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            Some(MapBrushEntity {
                model,
                mins: bounds.mins,
                maxs: bounds.maxs,
                train_corners,
                spawn_vars,
            })
        })
        .collect()
}

pub(in crate::scene) fn bsp_fx_runners(bsp: &Bsp, warnings: &mut Vec<String>) -> Vec<MapFxRunner> {
    let targets: BTreeMap<String, [f32; 3]> = bsp
        .entities
        .iter()
        .filter_map(|entity| {
            let name = bsp_entity_value(entity, b"targetname")?;
            let name = std::str::from_utf8(name).ok()?.trim();
            if name.is_empty() {
                return None;
            }
            let origin = bsp_entity_value(entity, b"origin").and_then(parse_bsp_entity_triplet)?;
            Some((name.to_owned(), origin))
        })
        .collect();

    bsp.entities
        .iter()
        .filter_map(|entity| {
            let classname = bsp_entity_value(entity, b"classname")?;
            if !classname.eq_ignore_ascii_case(b"fx_runner") {
                return None;
            }

            let origin = bsp_entity_value(entity, b"origin")
                .and_then(parse_bsp_entity_triplet)
                .unwrap_or([0.0; 3]);
            let Some(effect) = bsp_entity_value(entity, b"fxFile")
                .and_then(|value| std::str::from_utf8(value).ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                warnings.push(format!(
                    "fx_runner at {:.1} {:.1} {:.1} has no fxFile; skipped",
                    origin[0], origin[1], origin[2]
                ));
                return None;
            };

            let mut angles = bsp_entity_angles(entity);
            if angles == [0.0; 3] {
                // OpenJK SP_fx_runner defaults an unaimed runner to straight up.
                angles = [-90.0, 0.0, 0.0];
            }
            if let Some(target_name) = bsp_entity_value(entity, b"target")
                .and_then(|value| std::str::from_utf8(value).ok())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                if let Some(target) = targets.get(target_name) {
                    let direction = std::array::from_fn(|axis| target[axis] - origin[axis]);
                    let length_sq = direction.iter().map(|value| value * value).sum::<f32>();
                    if length_sq > f32::EPSILON {
                        angles = vector_to_jka_angles(direction);
                    }
                } else {
                    warnings.push(format!(
                        "fx_runner target '{target_name}' not found at {:.1} {:.1} {:.1}; using authored/default angles",
                        origin[0], origin[1], origin[2]
                    ));
                }
            }

            Some(MapFxRunner {
                effect: effect.replace('\\', "/"),
                origin,
                angles,
                delay_ms: parse_bsp_entity_i32(bsp_entity_value(entity, b"delay"), 200),
                random_ms: parse_bsp_entity_f32(bsp_entity_value(entity, b"random"), 0.0) as i32,
                spawnflags: parse_bsp_entity_i32(bsp_entity_value(entity, b"spawnflags"), 0),
            })
        })
        .collect()
}

pub(in crate::scene) fn bsp_dynamic_lights(bsp: &Bsp) -> Vec<DynamicLight> {
    bsp.entities
        .iter()
        .filter_map(|entity| {
            let classname = std::str::from_utf8(entity.get(b"classname")?).ok()?;
            let origin = entity
                .get(b"origin")
                .and_then(|value| std::str::from_utf8(value).ok());
            let color = entity
                .get(b"_color")
                .and_then(|value| std::str::from_utf8(value).ok());
            let brightness = entity
                .get(b"light")
                .and_then(|value| std::str::from_utf8(value).ok());
            dynamic_light_from_values(classname, origin, color, brightness)
        })
        .collect()
}

pub(in crate::scene) fn parse_positive_map_f32(
    entity: &jka_assets::map::MapEntity,
    key: &str,
) -> Option<f32> {
    entity
        .properties
        .get(key)
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
}

pub(in crate::scene) fn parse_map_f32(
    entity: &jka_assets::map::MapEntity,
    key: &str,
) -> Option<f32> {
    entity
        .properties
        .get(key)
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| value.is_finite())
}

pub(in crate::scene) fn map_dynamic_light(
    entity: &jka_assets::map::MapEntity,
) -> Option<DynamicLight> {
    let classname = entity.classname()?;
    if classname != "light" && classname != "lightJunior" {
        return None;
    }
    let origin = parse_light_triplet(entity.properties.get("origin")?)?;
    let spawnflags = entity
        .properties
        .get("spawnflags")
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(0);

    // q3map2's ColorNormalize divides by the largest component, not vector
    // length. Spawnflag 32 (NRC's "unnormalized") keeps the authored values.
    // The active JA compile profile reports `_color colorspace: linear`, so no
    // sRGB conversion belongs in this source-map preview path.
    let color = entity
        .properties
        .get("_color")
        .and_then(|value| parse_light_triplet(value))
        .map(Vec3::from_array)
        .map(|color| color.max(Vec3::ZERO))
        .map(|color| {
            if spawnflags & 32 != 0 {
                color
            } else {
                q3map_color_normalize(color)
            }
        })
        .unwrap_or(Vec3::ONE)
        .to_array();

    // q3map2 priority/defaults: `_light`, then `light`, default 300. `scale`
    // multiplies authored intensity, then ordinary point lights multiply by the
    // game pointScale (7500 for the JA/Q3 profile) to become compiler photons.
    let authored = parse_positive_map_f32(entity, "_light")
        .or_else(|| parse_positive_map_f32(entity, "light"))
        .unwrap_or(300.0);
    let scale = parse_map_f32(entity, "scale")
        .filter(|value| *value != 0.0)
        .unwrap_or(1.0);
    let brightness = (authored * scale).max(0.0);
    let photons = brightness * Q3MAP_POINT_SCALE;

    let linear = spawnflags & 1 != 0;
    let fade = if linear {
        parse_map_f32(entity, "fade")
            .filter(|value| *value != 0.0)
            .unwrap_or(1.0)
            .max(1.0e-5)
    } else {
        1.0
    };
    let angle_scale = parse_map_f32(entity, "_anglescale").unwrap_or(0.0);
    // In Q3/JKA, linear (spawnflag 1) disables angle attenuation, spawnflag 2
    // also disables it, while an explicit _anglescale re-enables it.
    let angle_attenuation = angle_scale != 0.0 || (!linear && spawnflags & 2 == 0);
    let extra_distance = parse_map_f32(entity, "_extradist").unwrap_or(0.0).abs();

    // q3map2's envelope is a culling optimization. It must not be multiplied
    // into the light curve. `-fast` drops contributions <= falloffTolerance (1),
    // so inverse-square point lights become irrelevant at sqrt(photons / 1).
    // Linear lights naturally reach zero at photons*linearScale/fade. The 16u
    // minimum mirrors q3map2's hot-spot distance clamp.
    let radius = if linear {
        (photons * Q3MAP_LINEAR_SCALE / fade).max(16.0)
    } else {
        (photons / Q3MAP_FALLOFF_TOLERANCE).sqrt().max(16.0)
    };

    // q3map2 accumulates direct-light values in lightmap-byte space. Divide the
    // photons once here so the shader's resulting diffuse factor is equivalent
    // to sampling the compiled lightmap as normalized 0..1 RGB.
    let intensity = photons / Q3MAP_LIGHTMAP_BYTE_SCALE;

    Some(DynamicLight {
        position: render_position(origin),
        color,
        radius,
        intensity,
        falloff: if linear {
            DynamicLightFalloff::Linear
        } else {
            DynamicLightFalloff::InverseSquare
        },
        surface_lighting: classname != "lightJunior",
        emitter_normal: [0.0; 3],
        emitter_two_sided: false,
        angle_attenuation,
        angle_scale,
        extra_distance,
    })
}

pub(in crate::scene) fn map_dynamic_lights(
    document: &jka_assets::map::MapDocument,
) -> Vec<DynamicLight> {
    document
        .entities
        .iter()
        .filter_map(map_dynamic_light)
        .collect()
}

pub(in crate::scene) fn load_movement(
    assets: &mut AssetSearchPath,
    warnings: &mut Vec<String>,
) -> Option<jka_movement::PmoveContext> {
    let result = assets
        .read("models/players/_humanoid/animation.cfg", 60_000)
        .map_err(|e| e.to_string())
        .and_then(|asset| {
            asset.ok_or_else(|| "Missing models/players/_humanoid/animation.cfg".to_string())
        })
        .and_then(|asset| jka_movement::PmoveContext::new(&asset.bytes));
    match result {
        Ok(movement) => Some(movement),
        Err(error) => {
            warnings.push(format!("Player animation timings unavailable: {error}"));
            None
        }
    }
}

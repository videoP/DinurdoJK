//! Source map geometry.
use crate::scene::{
    face_uv, map_vec, render_position, BTreeMap, DVec3, GpuVertex, MapDocument, MapFace, Shader,
    SpawnPoint, StageTexture, SurfaceMaterial, Textures, FALLBACK_TEXTURE_SIZE,
};

pub(in crate::scene) fn parse_map_triplet(value: Option<&String>) -> Option<[f32; 3]> {
    let mut values = value?.split_whitespace().map(str::parse::<f32>);
    let result = [
        values.next()?.ok()?,
        values.next()?.ok()?,
        values.next()?.ok()?,
    ];
    (values.next().is_none() && result.iter().all(|value| value.is_finite())).then_some(result)
}

/// FFA spawns only (`info_player_start` is an `info_player_deathmatch` alias);
/// duel/siege spots are used only when a map has nothing else.
pub(in crate::scene) fn map_spawn_points(document: &MapDocument) -> Vec<SpawnPoint> {
    let ffa = map_spawn_points_of(document, |classname| {
        matches!(classname, "info_player_deathmatch" | "info_player_start")
    });
    if !ffa.is_empty() {
        return ffa;
    }
    map_spawn_points_of(document, |classname| {
        matches!(
            classname,
            "info_player_duel" | "info_player_siegeteam1" | "info_player_siegeteam2"
        )
    })
}

pub(in crate::scene) fn map_spawn_points_of(
    document: &MapDocument,
    accepts: impl Fn(&str) -> bool,
) -> Vec<SpawnPoint> {
    document
        .entities
        .iter()
        .filter_map(|entity| {
            if !entity.classname().is_some_and(&accepts) {
                return None;
            }
            let mut origin = parse_map_triplet(entity.properties.get("origin"))?;
            // BSP spawn preparation applies the same small floor nudge used by JKA.
            origin[2] += 9.0;
            let yaw = entity
                .properties
                .get("angle")
                .and_then(|value| value.parse::<f32>().ok())
                .filter(|value| value.is_finite())
                .or_else(|| {
                    parse_map_triplet(entity.properties.get("angles")).map(|angles| angles[1])
                })
                .unwrap_or(0.0);
            let integer = |key: &str| {
                entity
                    .properties
                    .get(key)
                    .and_then(|value| value.trim().parse::<i32>().ok())
                    .unwrap_or(0)
            };
            Some(SpawnPoint {
                position: render_position(origin),
                yaw: yaw.to_radians(),
                initial: integer("spawnflags") & 1 != 0,
                no_humans: integer("nohumans") != 0,
            })
        })
        .collect()
}

pub(in crate::scene) fn canonical_map_shader(
    name: &str,
    library: &BTreeMap<String, Shader>,
) -> String {
    let raw = name.replace('\\', "/").to_ascii_lowercase();
    if library.contains_key(&raw) || raw.starts_with("textures/") {
        return raw;
    }
    // Loose map writers commonly omit `textures/`; direct image lookup still
    // needs the package-relative texture path even without a shader script.
    format!("textures/{raw}")
}

pub(in crate::scene) fn material_uv_size(
    material: &SurfaceMaterial,
    textures: &Textures,
) -> [f64; 2] {
    material
        .stages
        .iter()
        .find_map(|stage| match stage.texture {
            StageTexture::Image(index) => textures
                .images
                .get(index)
                .map(|image| [f64::from(image.width), f64::from(image.height)]),
            StageTexture::Lightmap | StageTexture::White => None,
        })
        .unwrap_or([FALLBACK_TEXTURE_SIZE; 2])
}

pub(in crate::scene) fn add_bounds(bounds: &mut Option<(DVec3, DVec3)>, point: DVec3) {
    match bounds {
        Some((minimum, maximum)) => {
            *minimum = minimum.min(point);
            *maximum = maximum.max(point);
        }
        None => *bounds = Some((point, point)),
    }
}

pub(in crate::scene) fn push_face_triangles(
    output: &mut Vec<GpuVertex>,
    face: &MapFace,
    polygon: &[DVec3],
    texture_size: [f64; 2],
) -> usize {
    if polygon.len() < 3 {
        return 0;
    }
    let normal = map_vec(face.plane.normal);
    let render_normal = render_position(face.plane.normal.map(|value| value as f32));
    let mut triangles = 0;
    for index in 1..polygon.len() - 1 {
        let points = [polygon[0], polygon[index], polygon[index + 1]];
        let area = (points[1] - points[0]).cross(points[2] - points[0]);
        if area.length_squared() < 1e-12 {
            continue;
        }
        let points = if area.dot(normal) >= 0.0 {
            points
        } else {
            [points[0], points[2], points[1]]
        };
        for point in points {
            output.push(GpuVertex {
                position: render_position(point.to_array().map(|value| value as f32)),
                uv: face_uv(face, point, texture_size),
                lightmap_uv: [-1.0, -1.0],
                normal: render_normal,
                color: [1.0; 4],
                alpha_cutoff: 1.0,
            });
        }
        triangles += 1;
    }
    triangles
}

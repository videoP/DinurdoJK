//! Source map brushes.
use crate::scene::{
    canonical_map_shader, Arc, BTreeMap, DVec3, GpuVertex, MapBrush, MapDocument, MapFace,
    MapJobPool, MapPlane, PlanarGroupKey, Shader, SurfaceMaterial, Task, TextureProjection,
};

#[derive(Debug)]
pub(crate) struct ReconstructedFace {
    pub(crate) face_index: usize,
    pub(crate) vertices: Vec<DVec3>,
}

#[derive(Debug)]
pub(crate) struct ReconstructedBrush {
    pub(in crate::scene) vertices: Vec<DVec3>,
    pub(crate) faces: Vec<ReconstructedFace>,
}

#[derive(Default)]
pub(in crate::scene) struct MapGeometry {
    pub(in crate::scene) vertices: Vec<GpuVertex>,
}

pub(in crate::scene) const SOURCE_MAP_CHUNK_SIZE: f64 = 1024.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::scene) struct MapChunkKey {
    pub(in crate::scene) x: i32,
    pub(in crate::scene) y: i32,
    pub(in crate::scene) z: i32,
}

pub(in crate::scene) fn map_chunk_key(vertices: &[DVec3]) -> MapChunkKey {
    let center = if vertices.is_empty() {
        DVec3::ZERO
    } else {
        vertices.iter().copied().sum::<DVec3>() / vertices.len() as f64
    };
    let coord = |value: f64| (value / SOURCE_MAP_CHUNK_SIZE).floor() as i32;
    MapChunkKey {
        x: coord(center.x),
        y: coord(center.y),
        z: coord(center.z),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::scene) struct MapGroupKey {
    pub(in crate::scene) material: usize,
    // Transparent/authored-portal geometry keeps its old per-face submission
    // isolation. Ordinary opaque geometry stays global unless GPU-driven source
    // batching was requested; environment-planar groups are spatial regardless
    // because the old source path isolated every one of those faces anyway.
    pub(in crate::scene) transparent_order: usize,
    pub(in crate::scene) chunk: Option<MapChunkKey>,
    pub(in crate::scene) planar_group: Option<PlanarGroupKey>,
}

pub(in crate::scene) struct MapMaterial {
    pub(in crate::scene) material: SurfaceMaterial,
    pub(in crate::scene) uv_size: [f64; 2],
}

#[derive(Debug)]
pub(in crate::scene) struct ReconstructedMapBrush {
    pub(in crate::scene) entity_index: usize,
    pub(in crate::scene) brush_index: usize,
    pub(in crate::scene) result: Result<ReconstructedBrush, String>,
}

pub(in crate::scene) fn map_vec(value: [f64; 3]) -> DVec3 {
    DVec3::new(value[0], value[1], value[2])
}

pub(in crate::scene) fn plane_intersection(
    a: &MapPlane,
    b: &MapPlane,
    c: &MapPlane,
) -> Option<DVec3> {
    let an = map_vec(a.normal);
    let bn = map_vec(b.normal);
    let cn = map_vec(c.normal);
    let b_cross_c = bn.cross(cn);
    let determinant = an.dot(b_cross_c);
    if determinant.abs() < 1e-6 {
        return None;
    }
    let point = (a.distance * b_cross_c + b.distance * cn.cross(an) + c.distance * an.cross(bn))
        / determinant;
    (point.x.is_finite() && point.y.is_finite() && point.z.is_finite()).then_some(point)
}

pub(in crate::scene) fn point_inside_brush(point: DVec3, brush: &MapBrush) -> bool {
    brush.faces.iter().all(|face| {
        map_vec(face.plane.normal).dot(point) - face.plane.distance <= BRUSH_INSIDE_EPSILON
    })
}

pub(in crate::scene) fn deduplicate_vertex(vertices: &mut Vec<DVec3>, point: DVec3) {
    let epsilon_sq = BRUSH_VERTEX_EPSILON * BRUSH_VERTEX_EPSILON;
    if !vertices
        .iter()
        .any(|existing| existing.distance_squared(point) <= epsilon_sq)
    {
        vertices.push(point);
    }
}

pub(in crate::scene) fn sort_face_vertices(vertices: &mut [DVec3], normal: DVec3) {
    let center = vertices.iter().copied().sum::<DVec3>() / vertices.len() as f64;
    let helper = if normal.z.abs() < 0.9 {
        DVec3::Z
    } else {
        DVec3::Y
    };
    let tangent = helper.cross(normal).normalize_or_zero();
    let bitangent = normal.cross(tangent);
    vertices.sort_by(|a, b| {
        let da = *a - center;
        let db = *b - center;
        let aa = da.dot(bitangent).atan2(da.dot(tangent));
        let ab = db.dot(bitangent).atan2(db.dot(tangent));
        aa.total_cmp(&ab)
    });

    if vertices.len() >= 3 {
        let triangle_normal = (vertices[1] - vertices[0]).cross(vertices[2] - vertices[0]);
        if triangle_normal.dot(normal) < 0.0 {
            vertices.reverse();
        }
    }
}

pub(in crate::scene) fn brush_has_unbounded_direction(brush: &MapBrush) -> bool {
    const DIRECTION_EPSILON: f64 = 1e-8;
    for i in 0..brush.faces.len().saturating_sub(1) {
        let a = map_vec(brush.faces[i].plane.normal);
        for j in i + 1..brush.faces.len() {
            let direction = a.cross(map_vec(brush.faces[j].plane.normal));
            let length_squared = direction.length_squared();
            if length_squared <= DIRECTION_EPSILON * DIRECTION_EPSILON {
                continue;
            }
            let direction = direction / length_squared.sqrt();
            for candidate in [direction, -direction] {
                if brush
                    .faces
                    .iter()
                    .all(|face| map_vec(face.plane.normal).dot(candidate) <= DIRECTION_EPSILON)
                {
                    return true;
                }
            }
        }
    }
    false
}

pub(crate) fn reconstruct_brush(brush: &MapBrush) -> Result<ReconstructedBrush, String> {
    if brush.faces.len() < 4 {
        return Err("fewer than four valid planes".into());
    }
    if brush_has_unbounded_direction(brush) {
        return Err("plane half-spaces do not form a bounded volume".into());
    }

    let mut vertices = Vec::new();
    for i in 0..brush.faces.len() - 2 {
        for j in i + 1..brush.faces.len() - 1 {
            for k in j + 1..brush.faces.len() {
                let Some(point) = plane_intersection(
                    &brush.faces[i].plane,
                    &brush.faces[j].plane,
                    &brush.faces[k].plane,
                ) else {
                    continue;
                };
                if point_inside_brush(point, brush) {
                    deduplicate_vertex(&mut vertices, point);
                }
            }
        }
    }
    if vertices.len() < 4 {
        return Err(format!(
            "only {} bounded corner(s) reconstructed",
            vertices.len()
        ));
    }

    let mut faces = Vec::new();
    for (face_index, face) in brush.faces.iter().enumerate() {
        let normal = map_vec(face.plane.normal);
        let mut face_vertices: Vec<_> = vertices
            .iter()
            .copied()
            .filter(|point| (normal.dot(*point) - face.plane.distance).abs() <= BRUSH_FACE_EPSILON)
            .collect();
        if face_vertices.len() < 3 {
            return Err(format!(
                "face {face_index} does not bound a polygon ({} on-plane corners)",
                face_vertices.len()
            ));
        }
        sort_face_vertices(&mut face_vertices, normal);
        faces.push(ReconstructedFace {
            face_index,
            vertices: face_vertices,
        });
    }
    if faces.len() < 4 {
        return Err(format!(
            "only {} face polygon(s) reconstructed",
            faces.len()
        ));
    }

    Ok(ReconstructedBrush { vertices, faces })
}

pub(in crate::scene) fn reconstruct_source_brushes(
    document: Arc<MapDocument>,
    static_entity_indices: &[usize],
    jobs: Option<&MapJobPool>,
) -> Result<Vec<ReconstructedMapBrush>, String> {
    let work = static_entity_indices
        .iter()
        .flat_map(|&entity_index| {
            (0..document.entities[entity_index].brushes.len())
                .map(move |brush_index| (entity_index, brush_index))
        })
        .collect::<Vec<_>>();

    if work.len() < 256 || jobs.is_none() {
        return Ok(work
            .into_iter()
            .map(|(entity_index, brush_index)| ReconstructedMapBrush {
                entity_index,
                brush_index,
                result: reconstruct_brush(&document.entities[entity_index].brushes[brush_index]),
            })
            .collect());
    }

    let jobs = jobs.expect("source reconstruction jobs");
    let job_count = (jobs.worker_count().saturating_mul(2)).clamp(1, work.len());
    let chunk_size = work.len().div_ceil(job_count);
    let mut handles = Vec::new();
    for chunk in work.chunks(chunk_size) {
        let document = Arc::clone(&document);
        let chunk = chunk.to_vec();
        handles.push(jobs.submit(Task::MapPrepare, move || {
            chunk
                .into_iter()
                .map(|(entity_index, brush_index)| ReconstructedMapBrush {
                    entity_index,
                    brush_index,
                    result: reconstruct_brush(
                        &document.entities[entity_index].brushes[brush_index],
                    ),
                })
                .collect::<Vec<_>>()
        })?);
    }

    let mut reconstructed = Vec::with_capacity(work.len());
    for handle in handles {
        reconstructed.extend(handle.join()?);
    }
    // Worker completion order must not affect transparent ordering, warning order,
    // or generated vertex ranges. Preserve the original entity/brush order.
    reconstructed.sort_by_key(|item| (item.entity_index, item.brush_index));
    Ok(reconstructed)
}

pub(in crate::scene) fn legacy_texture_axes(normal: DVec3) -> (DVec3, DVec3) {
    const AXES: [([f64; 3], [f64; 3], [f64; 3]); 6] = [
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
        ([0.0, 0.0, -1.0], [1.0, 0.0, 0.0], [0.0, -1.0, 0.0]),
        ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]),
        ([-1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
    ];
    // Match q3map's TextureAxisFromPlane exactly: ties keep the earlier axis.
    let mut best = 0.0;
    let mut best_axis = 0usize;
    for (index, (axis, _, _)) in AXES.iter().enumerate() {
        let candidate = map_vec(*axis).dot(normal);
        if candidate > best {
            best = candidate;
            best_axis = index;
        }
    }
    (map_vec(AXES[best_axis].1), map_vec(AXES[best_axis].2))
}

pub(in crate::scene) fn quake_sin_cos(degrees: f64) -> (f64, f64) {
    match degrees {
        0.0 => (0.0, 1.0),
        90.0 => (1.0, 0.0),
        180.0 => (0.0, -1.0),
        270.0 => (-1.0, 0.0),
        _ => degrees.to_radians().sin_cos(),
    }
}

pub(in crate::scene) fn brush_primitive_axes(normal: DVec3) -> (DVec3, DVec3) {
    // q3map2/GtkRadiant brush-primitives basis: rotate world space around Z and
    // then Y so the local Z axis aligns with the face normal. The stored 2x3
    // matrix maps the resulting local X/Y coordinates directly to normalized UV.
    let theta_z = if normal.x.abs() > 1e-9 || normal.y.abs() > 1e-9 {
        normal.y.atan2(normal.x)
    } else {
        0.0
    };
    let theta_y = normal
        .z
        .atan2((normal.x * normal.x + normal.y * normal.y).sqrt());
    let x_axis = DVec3::new(-theta_z.sin(), theta_z.cos(), 0.0);
    let y_axis = DVec3::new(
        theta_y.sin() * theta_z.cos(),
        theta_y.sin() * theta_z.sin(),
        -theta_y.cos(),
    );
    (x_axis, y_axis)
}

pub(in crate::scene) fn safe_scale(value: f64) -> f64 {
    if value.abs() < 1e-9 {
        1.0
    } else {
        value
    }
}

pub(in crate::scene) fn face_uv(face: &MapFace, point: DVec3, texture_size: [f64; 2]) -> [f32; 2] {
    let width = texture_size[0].max(1.0);
    let height = texture_size[1].max(1.0);
    let normal = map_vec(face.plane.normal);
    let uv = match &face.projection {
        TextureProjection::Legacy {
            shift,
            rotate,
            scale,
        } => {
            let (base_u, base_v) = legacy_texture_axes(normal);
            let (sin, cos) = quake_sin_cos(*rotate);
            let u_axis = base_u * cos - base_v * sin;
            let v_axis = base_u * sin + base_v * cos;
            [
                (point.dot(u_axis) / safe_scale(scale[0]) + shift[0]) / width,
                (point.dot(v_axis) / safe_scale(scale[1]) + shift[1]) / height,
            ]
        }
        TextureProjection::Valve220 {
            u_axis,
            u_shift,
            v_axis,
            v_shift,
            scale,
            ..
        } => [
            (point.dot(map_vec(*u_axis)) / safe_scale(scale[0]) + *u_shift) / width,
            (point.dot(map_vec(*v_axis)) / safe_scale(scale[1]) + *v_shift) / height,
        ],
        TextureProjection::BrushPrimitive { matrix } => {
            let (x_axis, y_axis) = brush_primitive_axes(normal);
            let x = point.dot(x_axis);
            let y = point.dot(y_axis);
            [
                matrix[0][0] * x + matrix[0][1] * y + matrix[0][2],
                matrix[1][0] * x + matrix[1][1] * y + matrix[1][2],
            ]
        }
    };
    [uv[0] as f32, uv[1] as f32]
}

pub(in crate::scene) fn is_utility_shader(name: &str) -> bool {
    let normalized = name.replace('\\', "/").to_ascii_lowercase();
    let normalized = normalized.strip_prefix("textures/").unwrap_or(&normalized);
    let leaf = normalized.rsplit('/').next().unwrap_or(normalized);
    matches!(
        leaf,
        "caulk"
            | "nodraw"
            | "clip"
            | "playerclip"
            | "monsterclip"
            | "botclip"
            | "weaponclip"
            | "trigger"
            | "hint"
            | "skip"
            | "areaportal"
            | "origin"
            | "lightgrid"
            | "cushion"
            | "fog"
    ) || leaf.starts_with("caulk")
        || leaf.starts_with("nodraw")
        || leaf.starts_with("trigger_")
        || leaf.starts_with("clip_")
        || leaf.ends_with("_clip")
}

/// Shader-script metadata is authoritative for utility-only surfaces. A custom
/// shader does not have to be named `caulk`/`playerclip`/etc, so filename
/// heuristics alone can accidentally send compiler-only faces to WGPU. Keep
/// visible nonsolid materials (glass/water/etc.) out of this test.
pub(in crate::scene) fn shader_is_render_utility(shader: &Shader) -> bool {
    shader.nodraw
        || shader.player_clip
        || shader.monster_clip
        || shader.bot_clip
        || shader.shot_clip
        || shader.trigger
        || shader.fog
}

pub(in crate::scene) const SOURCE_CONTENTS_SOLID: u32 = 0x0000_0001;

pub(in crate::scene) const SOURCE_CONTENTS_LAVA: u32 = 0x0000_0002;

pub(in crate::scene) const SOURCE_CONTENTS_WATER: u32 = 0x0000_0004;

pub(in crate::scene) const SOURCE_CONTENTS_FOG: u32 = 0x0000_0008;

pub(in crate::scene) const SOURCE_CONTENTS_PLAYERCLIP: u32 = 0x0000_0010;

pub(in crate::scene) const SOURCE_CONTENTS_MONSTERCLIP: u32 = 0x0000_0020;

pub(in crate::scene) const SOURCE_CONTENTS_BOTCLIP: u32 = 0x0000_0040;

pub(in crate::scene) const SOURCE_CONTENTS_SHOTCLIP: u32 = 0x0000_0080;

pub(in crate::scene) const SOURCE_CONTENTS_TRIGGER: u32 = 0x0000_0400;

pub(in crate::scene) const SOURCE_CONTENTS_OPAQUE: u32 = 0x0000_8000;

pub(in crate::scene) const SOURCE_SURF_SKY: u32 = 0x0000_2000;

pub(in crate::scene) const SOURCE_SURF_SLICK: u32 = 0x0000_4000;

pub(in crate::scene) const SOURCE_SURF_METALSTEPS: u32 = 0x0000_8000;

pub(in crate::scene) const SOURCE_SURF_NODAMAGE: u32 = 0x0004_0000;

pub(in crate::scene) const SOURCE_SURF_NODRAW: u32 = 0x0020_0000;

pub(in crate::scene) const SOURCE_SURF_NOSTEPS: u32 = 0x0040_0000;

pub(in crate::scene) const SOURCE_SURF_NOMISCENTS: u32 = 0x0100_0000;

pub(in crate::scene) const SOURCE_SURF_BEVELS_MASK: u32 = SOURCE_SURF_SLICK
    | SOURCE_SURF_METALSTEPS
    | SOURCE_SURF_NODAMAGE
    | SOURCE_SURF_NOSTEPS
    | SOURCE_SURF_NOMISCENTS;

pub(in crate::scene) fn source_shader_leaf(name: &str) -> String {
    let normalized = name.replace('\\', "/").to_ascii_lowercase();
    normalized
        .rsplit('/')
        .next()
        .unwrap_or(&normalized)
        .to_string()
}

pub(in crate::scene) fn source_face_collision_flags(
    face: &MapFace,
    shader: Option<&Shader>,
) -> (i32, i32) {
    let leaf = source_shader_leaf(&face.shader);
    // q3map2's JA/SOF2 table applies this default before shader surfaceParms.
    let mut contents = SOURCE_CONTENTS_SOLID | SOURCE_CONTENTS_OPAQUE;
    let mut surface_flags = 0u32;

    if let Some(shader) = shader {
        contents &= !shader.collision_contents_clear;
        contents |= shader.collision_contents_add;
        surface_flags &= !shader.collision_surface_flags_clear;
        surface_flags |= shader.collision_surface_flags_add;

        // skyparms can identify a sky shader even when an unusual script omits
        // `surfaceparm sky`; preserve the gameplay surface bit in that case.
        if shader.sky {
            surface_flags |= SOURCE_SURF_SKY;
        }
        if shader.slick {
            surface_flags |= SOURCE_SURF_SLICK;
        }
        if shader.nodraw {
            surface_flags |= SOURCE_SURF_NODRAW;
        }
    } else {
        // Fallback only when no shader script exists. These mirror the normal
        // JKA tool-texture intent closely enough that loose source maps remain
        // playable; a loaded shader always wins over filename heuristics.
        match leaf.as_str() {
            "clip" | "playerclip" => {
                contents = SOURCE_CONTENTS_PLAYERCLIP;
                surface_flags |= SOURCE_SURF_NODRAW;
            }
            "monsterclip" => {
                contents = SOURCE_CONTENTS_MONSTERCLIP;
                surface_flags |= SOURCE_SURF_NODRAW;
            }
            "botclip" => {
                contents = SOURCE_CONTENTS_BOTCLIP;
                surface_flags |= SOURCE_SURF_NODRAW;
            }
            "weaponclip" | "shotclip" => {
                contents = SOURCE_CONTENTS_SHOTCLIP;
                surface_flags |= SOURCE_SURF_NODRAW;
            }
            "trigger" => {
                contents = SOURCE_CONTENTS_TRIGGER;
                surface_flags |= SOURCE_SURF_NODRAW;
            }
            "water" => contents = SOURCE_CONTENTS_WATER | SOURCE_CONTENTS_OPAQUE,
            "lava" => contents = SOURCE_CONTENTS_LAVA | SOURCE_CONTENTS_OPAQUE,
            // Raven's SOF2/JKA SLIME bit is also used as projectileclip.
            "slime" => contents = 0x0002_0000 | SOURCE_CONTENTS_OPAQUE,
            "fog" => contents = SOURCE_CONTENTS_FOG,
            "nodraw" => surface_flags |= SOURCE_SURF_NODRAW,
            _ if leaf.starts_with("caulk") || leaf.starts_with("nodraw") => {
                surface_flags |= SOURCE_SURF_NODRAW;
            }
            _ => {}
        }
    }

    // Legacy .map writers may append q3map contents/surfaceFlags/value after
    // the texture projection. Radiant/q3map writers commonly store compile
    // modifiers such as CONTENTS_DETAIL there without repeating the material
    // contents already deduced from the shader. Treating a modifier as the
    // complete gameplay contents makes ordinary detail floors non-solid. Merge
    // modifier-only values; replace only when the explicit bits actually name a
    // gameplay volume/collision class.
    if let Some(value) = face
        .trailing
        .first()
        .copied()
        .filter(|value| value.is_finite())
    {
        let explicit = value.round() as i64 as u32;
        if explicit != 0 {
            const EXPLICIT_GAMEPLAY_CLASS: u32 = SOURCE_CONTENTS_SOLID
                | SOURCE_CONTENTS_LAVA
                | SOURCE_CONTENTS_WATER
                | SOURCE_CONTENTS_FOG
                | SOURCE_CONTENTS_PLAYERCLIP
                | SOURCE_CONTENTS_MONSTERCLIP
                | SOURCE_CONTENTS_BOTCLIP
                | SOURCE_CONTENTS_SHOTCLIP
                | SOURCE_CONTENTS_TRIGGER
                | 0x0000_1000 // CONTENTS_TERRAIN
                | 0x0002_0000; // CONTENTS_SLIME
            if explicit & EXPLICIT_GAMEPLAY_CLASS == 0 {
                // Compile modifiers (DETAIL, TRANSLUCENT, etc.) augment the
                // contents already deduced from the material. This also preserves
                // an authored surfaceParm nonsolid clear from the shader parser.
                contents |= explicit;
            } else {
                // Actual gameplay content classes are authoritative: playerclip,
                // liquids, trigger volumes, explicit solid, and so on must not
                // inherit ordinary SOLID merely because the face has a texture.
                contents = explicit;
            }
        }
    }
    if let Some(value) = face
        .trailing
        .get(1)
        .copied()
        .filter(|value| value.is_finite())
    {
        let explicit = value.round() as i64 as u32;
        if explicit != 0 {
            surface_flags = explicit;
        }
    }
    (contents as i32, surface_flags as i32)
}

pub(in crate::scene) fn push_source_collision_plane(
    planes: &mut Vec<jka_movement::SourceCollisionPlane>,
    normal: DVec3,
    distance: f64,
    surface_flags: i32,
) {
    let length = normal.length();
    if !length.is_finite() || length < 1e-9 || !distance.is_finite() {
        return;
    }
    let normal = normal / length;
    let distance = distance / length;
    let n = [normal.x as f32, normal.y as f32, normal.z as f32];
    let d = distance as f32;
    if let Some(existing) = planes.iter_mut().find(|plane| {
        (plane.normal[0] - n[0]).abs() < 1e-4
            && (plane.normal[1] - n[1]).abs() < 1e-4
            && (plane.normal[2] - n[2]).abs() < 1e-4
            && (plane.distance - d).abs() < 0.05
    }) {
        // q3map2 ORs the bevel-relevant surface flags when a generated bevel
        // resolves to an already-present plane.
        existing.surface_flags |= surface_flags;
    } else {
        planes.push(jka_movement::SourceCollisionPlane {
            normal: n,
            distance: d,
            surface_flags,
        });
    }
}

pub(in crate::scene) fn source_collision_brush(
    brush: &MapBrush,
    reconstructed: &ReconstructedBrush,
    library: &BTreeMap<String, Shader>,
) -> Option<jka_movement::SourceCollisionBrush> {
    if reconstructed.vertices.len() < 4 {
        return None;
    }
    let mut minimum = DVec3::splat(f64::INFINITY);
    let mut maximum = DVec3::splat(f64::NEG_INFINITY);
    for &point in &reconstructed.vertices {
        minimum = minimum.min(point);
        maximum = maximum.max(point);
    }

    let mut planes = Vec::new();
    let mut contents = 0i32;
    let mut face_surface_flags = Vec::with_capacity(brush.faces.len());
    for face in &brush.faces {
        let shader_name = canonical_map_shader(&face.shader, library);
        let (face_contents, surface_flags) =
            source_face_collision_flags(face, library.get(&shader_name));
        contents |= face_contents;
        face_surface_flags.push(surface_flags);
        push_source_collision_plane(
            &mut planes,
            map_vec(face.plane.normal),
            face.plane.distance,
            surface_flags,
        );
    }
    if contents == 0 || planes.len() < 4 {
        return None;
    }

    // q3map2 adds axial and edge bevels to collision brushes. Face planes alone
    // are sufficient for point traces, but a swept player AABB needs these
    // additional Minkowski planes around slanted brush edges to match CM.
    const BEVEL_EPSILON: f64 = 0.1;
    for axis in 0..3 {
        let axial_flags = |coordinate: f64| -> i32 {
            reconstructed
                .faces
                .iter()
                .filter(|polygon| {
                    polygon
                        .vertices
                        .iter()
                        .any(|vertex| (vertex[axis] - coordinate).abs() < BEVEL_EPSILON)
                })
                .fold(0u32, |flags, polygon| {
                    flags
                        | (face_surface_flags
                            .get(polygon.face_index)
                            .copied()
                            .unwrap_or_default() as u32
                            & SOURCE_SURF_BEVELS_MASK)
                }) as i32
        };
        let mut positive = DVec3::ZERO;
        positive[axis] = 1.0;
        push_source_collision_plane(
            &mut planes,
            positive,
            maximum[axis],
            axial_flags(maximum[axis]),
        );
        push_source_collision_plane(
            &mut planes,
            -positive,
            -minimum[axis],
            axial_flags(minimum[axis]),
        );
    }
    const MAX_SOURCE_COLLISION_PLANES: usize = 256;
    'faces: for polygon in &reconstructed.faces {
        if polygon.vertices.len() < 2 {
            continue;
        }
        for edge_index in 0..polygon.vertices.len() {
            let a = polygon.vertices[edge_index];
            let b = polygon.vertices[(edge_index + 1) % polygon.vertices.len()];
            let edge = b - a;
            let edge_length = edge.length();
            if edge_length < 1e-6 {
                continue;
            }
            let edge = edge / edge_length;
            if edge.x.abs() > 0.9999 || edge.y.abs() > 0.9999 || edge.z.abs() > 0.9999 {
                continue;
            }
            for axis in 0..3 {
                for sign in [-1.0, 1.0] {
                    let mut axis_vector = DVec3::ZERO;
                    axis_vector[axis] = sign;
                    let candidate = edge.cross(axis_vector);
                    let length = candidate.length();
                    if length < 1e-6 {
                        continue;
                    }
                    let normal = candidate / length;
                    let distance = normal.dot(a);
                    let mut has_inside = false;
                    let valid = reconstructed.vertices.iter().all(|point| {
                        let delta = normal.dot(*point) - distance;
                        if delta < -BEVEL_EPSILON {
                            has_inside = true;
                        }
                        delta <= BEVEL_EPSILON
                    });
                    if valid && has_inside {
                        let bevel_surface_flags = face_surface_flags
                            .get(polygon.face_index)
                            .copied()
                            .unwrap_or_default()
                            as u32
                            & SOURCE_SURF_BEVELS_MASK;
                        push_source_collision_plane(
                            &mut planes,
                            normal,
                            distance,
                            bevel_surface_flags as i32,
                        );
                        if planes.len() >= MAX_SOURCE_COLLISION_PLANES {
                            break 'faces;
                        }
                    }
                }
            }
        }
    }

    Some(jka_movement::SourceCollisionBrush {
        planes,
        mins: [minimum.x as f32, minimum.y as f32, minimum.z as f32],
        maxs: [maximum.x as f32, maximum.y as f32, maximum.z as f32],
        contents,
    })
}

pub(in crate::scene) const BRUSH_INSIDE_EPSILON: f64 = 0.05;

pub(in crate::scene) const BRUSH_FACE_EPSILON: f64 = 0.1;

pub(in crate::scene) const BRUSH_VERTEX_EPSILON: f64 = 0.05;

pub(in crate::scene) const FALLBACK_TEXTURE_SIZE: f64 = 128.0;

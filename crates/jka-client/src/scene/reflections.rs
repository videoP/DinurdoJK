//! Reflections.
use crate::scene::{
    parse_light_triplet, render_position, AssetSearchPath, BlendMode, Bsp, DrawBatch, DrawClass,
    GpuVertex, MapDocument, PlanarGroupKey, ReflectionProbe, Vec3, REFLECTION_CACHE_PLANAR,
    REFLECTION_CACHE_PROBE, REFLECTION_CACHE_SSR,
};

#[derive(Debug, Clone)]
pub(in crate::scene) struct ReflectionProbeDescriptor {
    pub(in crate::scene) name: Option<String>,
    pub(in crate::scene) position: [f32; 3],
    pub(in crate::scene) radius: f32,
}

pub(in crate::scene) fn json_f32(value: &serde_json::Value) -> Option<f32> {
    let parsed = if let Some(number) = value.as_f64() {
        number as f32
    } else {
        value.as_str()?.trim().parse::<f32>().ok()?
    };
    parsed.is_finite().then_some(parsed)
}

pub(in crate::scene) fn json_vec3(value: &serde_json::Value) -> Option<[f32; 3]> {
    let values = value.as_array()?;
    if values.len() != 3 {
        return None;
    }
    Some([
        json_f32(&values[0])?,
        json_f32(&values[1])?,
        json_f32(&values[2])?,
    ])
}

pub(in crate::scene) fn reflection_probe_descriptors(
    bsp: &Bsp,
    assets: &mut AssetSearchPath,
    map_name: &str,
    warnings: &mut Vec<String>,
) -> Result<Vec<ReflectionProbeDescriptor>, String> {
    // Match Rend2/OpenGL2 exactly: cubemaps/<world baseName>/env.json takes
    // precedence over misc_cubemap entities.
    let env_path = format!("cubemaps/{map_name}/env.json");
    if let Some(asset) = assets
        .read(&env_path, 4 * 1024 * 1024)
        .map_err(|error| error.to_string())?
    {
        match serde_json::from_slice::<serde_json::Value>(&asset.bytes) {
            Ok(root) => {
                let Some(entries) = root.get("Cubemaps").and_then(serde_json::Value::as_array)
                else {
                    warnings.push(format!("{map_name}: {env_path} has no Cubemaps array"));
                    return Ok(Vec::new());
                };
                let mut probes = Vec::with_capacity(entries.len());
                for (index, entry) in entries.iter().enumerate() {
                    let Some(position) = entry.get("Position").and_then(json_vec3) else {
                        warnings.push(format!(
                            "{map_name}: ignored cubemap {index} in {env_path}: invalid Position"
                        ));
                        continue;
                    };
                    let radius = entry
                        .get("Radius")
                        .and_then(json_f32)
                        .filter(|value| *value > 0.0)
                        .unwrap_or(1000.0);
                    let name = entry
                        .get("Name")
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned);
                    probes.push(ReflectionProbeDescriptor {
                        name,
                        position,
                        radius,
                    });
                }
                warnings.push(format!(
                    "{map_name}: parsed {} reflection probe(s) from Rend2 {env_path}",
                    probes.len()
                ));
                return Ok(probes);
            }
            Err(error) => {
                warnings.push(format!("{map_name}: invalid Rend2 {env_path}: {error}"));
                return Ok(Vec::new());
            }
        }
    }

    let mut probes = Vec::new();
    for entity in &bsp.entities {
        let classname = entity
            .get(b"classname")
            .and_then(|value| std::str::from_utf8(value).ok());
        if !classname.is_some_and(|value| value.eq_ignore_ascii_case("misc_cubemap")) {
            continue;
        }
        let Some(position) = entity
            .get(b"origin")
            .and_then(|value| std::str::from_utf8(value).ok())
            .and_then(parse_light_triplet)
        else {
            continue;
        };
        let radius = entity
            .get(b"radius")
            .and_then(|value| std::str::from_utf8(value).ok())
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|value| value.is_finite() && *value > 0.0)
            .unwrap_or(1000.0);
        let name = entity
            .get(b"name")
            .and_then(|value| std::str::from_utf8(value).ok())
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
        probes.push(ReflectionProbeDescriptor {
            name,
            position,
            radius,
        });
    }
    if !probes.is_empty() {
        warnings.push(format!(
            "{map_name}: using {} misc_cubemap reflection probe entity/entities",
            probes.len()
        ));
    }
    Ok(probes)
}

pub(in crate::scene) fn cubemap_asset_name(
    map_name: &str,
    index: usize,
    name: Option<&str>,
) -> String {
    if let Some(name) = name {
        let normalized = name.trim().replace('\\', "/");
        let slash = normalized.rfind('/').map_or(0, |index| index + 1);
        let stem = normalized[slash..]
            .rfind('.')
            .map(|extension| &normalized[..slash + extension])
            .unwrap_or(normalized.as_str());
        return format!("{stem}.dds");
    }
    format!("cubemaps/{map_name}/{index:03}.dds")
}

pub(in crate::scene) fn load_reflection_probes(
    bsp: &Bsp,
    assets: &mut AssetSearchPath,
    map_name: &str,
    warnings: &mut Vec<String>,
) -> Result<Vec<ReflectionProbe>, String> {
    let descriptors = reflection_probe_descriptors(bsp, assets, map_name, warnings)?;
    let mut probes = Vec::new();
    for (index, descriptor) in descriptors.into_iter().enumerate() {
        let path = cubemap_asset_name(map_name, index, descriptor.name.as_deref());
        let Some(asset) = assets
            .read(&path, 128 * 1024 * 1024)
            .map_err(|error| error.to_string())?
        else {
            warnings.push(format!(
                "{map_name}: reflection probe image not found: {path}"
            ));
            continue;
        };
        let dds = match image_dds::ddsfile::Dds::read(std::io::Cursor::new(&asset.bytes)) {
            Ok(dds) => dds,
            Err(error) => {
                warnings.push(format!("{map_name}: failed to parse {path}: {error}"));
                continue;
            }
        };
        let decoded = match image_dds::SurfaceRgba8::decode_dds(&dds) {
            Ok(decoded) => decoded,
            Err(error) => {
                warnings.push(format!("{map_name}: failed to decode {path}: {error}"));
                continue;
            }
        };
        if decoded.layers != 6 || decoded.depth != 1 || decoded.width != decoded.height {
            warnings.push(format!(
                "{map_name}: ignored {path}: expected 6-face square cubemap, got {}x{} depth={} layers={}",
                decoded.width, decoded.height, decoded.depth, decoded.layers
            ));
            continue;
        }
        probes.push(ReflectionProbe {
            label: path,
            position: render_position(descriptor.position),
            radius: descriptor.radius,
            width: decoded.width,
            height: decoded.height,
            mip_level_count: decoded.mipmaps.max(1),
            rgba: decoded.data,
        });
    }
    if !probes.is_empty() {
        warnings.push(format!(
            "{map_name}: loaded {} Rend2 reflection cubemap(s)",
            probes.len()
        ));
    }
    Ok(probes)
}

pub(in crate::scene) fn assign_reflection_probes(
    batches: &mut [DrawBatch],
    vertices: &[GpuVertex],
    probes: &[ReflectionProbe],
) {
    if probes.is_empty() {
        return;
    }
    for batch in batches {
        if batch.pipeline.class == DrawClass::Sky
            || batch.texture_is_lightmap
            || batch.texture_is_white
        {
            continue;
        }
        let start = batch.vertices.start as usize;
        let end = batch.vertices.end as usize;
        let Some(slice) = vertices.get(start..end) else {
            continue;
        };
        if slice.is_empty() {
            continue;
        }
        let mut minimum = [f32::INFINITY; 3];
        let mut maximum = [f32::NEG_INFINITY; 3];
        for vertex in slice {
            for axis in 0..3 {
                minimum[axis] = minimum[axis].min(vertex.position[axis]);
                maximum[axis] = maximum[axis].max(vertex.position[axis]);
            }
        }
        let center: [f32; 3] = std::array::from_fn(|axis| (minimum[axis] + maximum[axis]) * 0.5);
        let Some((probe_index, probe)) = probes.iter().enumerate().min_by(|(_, a), (_, b)| {
            let distance = |probe: &ReflectionProbe| {
                (0..3)
                    .map(|axis| {
                        let delta = center[axis] - probe.position[axis];
                        delta * delta
                    })
                    .sum::<f32>()
            };
            distance(a).total_cmp(&distance(b))
        }) else {
            continue;
        };
        batch.reflection_probe = Some(probe_index);
        batch.reflection_probe_position_radius = [
            probe.position[0],
            probe.position[1],
            probe.position[2],
            probe.radius,
        ];
    }
}

pub(in crate::scene) fn cache_reflection_decisions(batches: &mut [DrawBatch]) {
    for batch in batches {
        batch.reflection_cache_flags = 0;
        batch.reflection_roughness_hint = 1.0;
        if batch.pipeline.class == DrawClass::Sky
            || batch.texture_is_lightmap
            || batch.texture_is_white
        {
            continue;
        }

        let has_pbr_companion = batch.normal_texture.is_some()
            || batch.roughness_texture.is_some()
            || batch.metallic_texture.is_some()
            || batch.specular_texture.is_some()
            || batch.roughness_override.is_some()
            || batch.specular_reflectance.is_some();
        let probe_eligible = batch.reflection_probe.is_some() && has_pbr_companion;
        let planar_eligible = batch.planar_reflection || batch.planar_environment_candidate;

        let roughness = batch.roughness_override.unwrap_or_else(|| {
            if batch.water {
                0.08
            } else if batch.planar_reflection {
                0.02
            } else if batch.planar_environment_candidate {
                0.22
            } else if batch.metallic_texture.is_some() || batch.specular_texture.is_some() {
                0.35
            } else if batch.roughness_texture.is_some() {
                0.50
            } else if batch.specular_reflectance.is_some() {
                0.45
            } else {
                0.85
            }
        });
        batch.reflection_roughness_hint = roughness.clamp(0.0, 1.0);

        // The map-load cache answers whether this geometry can participate in
        // the screen-space pass at all. Keep opaque/masked depth-writing world
        // surfaces eligible because rain can make an otherwise rough material
        // reflective at runtime; the SSR shader applies the quality/roughness
        // budget after evaluating current wetness. This preserves the existing
        // dynamic wet-surface behavior while still avoiding work on sky/blended
        // stages that cannot produce a stable depth-buffer reflection.
        let ssr_eligible = batch.pipeline.blend == BlendMode::Opaque
            && matches!(batch.pipeline.class, DrawClass::Opaque | DrawClass::Mask);

        if probe_eligible {
            batch.reflection_cache_flags |= REFLECTION_CACHE_PROBE;
        }
        if ssr_eligible {
            batch.reflection_cache_flags |= REFLECTION_CACHE_SSR;
        }
        if planar_eligible {
            batch.reflection_cache_flags |= REFLECTION_CACHE_PLANAR;
        }
    }
}

pub(in crate::scene) fn log_reflection_cache(name: &str, batches: &[DrawBatch]) {
    let probe = batches
        .iter()
        .filter(|batch| (batch.reflection_cache_flags & REFLECTION_CACHE_PROBE) != 0)
        .count();
    let ssr = batches
        .iter()
        .filter(|batch| (batch.reflection_cache_flags & REFLECTION_CACHE_SSR) != 0)
        .count();
    let planar = batches
        .iter()
        .filter(|batch| (batch.reflection_cache_flags & REFLECTION_CACHE_PLANAR) != 0)
        .count();
    devprintln!(
        2,
        "{name}: reflection cache: {probe} probe batch(es), {ssr} SSR-eligible batch(es), {planar} planar candidate batch(es)"
    );
}

pub(in crate::scene) const PORTAL_SURFACE_MAX_DISTANCE: f32 = 64.0;

#[derive(Clone, Copy)]
pub(in crate::scene) struct PortalSurfaceAnchor {
    pub(in crate::scene) origin: [f32; 3],
    pub(in crate::scene) mirror: bool,
}

pub(in crate::scene) const ENVIRONMENT_PLANAR_DISTANCE_EPSILON: f32 = 0.5;

pub(in crate::scene) const ENVIRONMENT_PLANAR_NORMAL_DOT: f32 = 0.999;

pub(in crate::scene) fn planar_group_key(vertices: &[GpuVertex]) -> Option<PlanarGroupKey> {
    let mut plane = planar_batch_plane(vertices, true)?;

    // The same geometric plane can be represented as (n, d) or (-n, -d).
    // Canonicalise the sign so opposite triangle winding does not fragment an
    // otherwise identical reflector. Reflection math is invariant to this flip.
    let flip = plane[0] < -1e-6
        || (plane[0].abs() <= 1e-6 && plane[1] < -1e-6)
        || (plane[0].abs() <= 1e-6 && plane[1].abs() <= 1e-6 && plane[2] < 0.0);
    if flip {
        for value in &mut plane {
            *value = -*value;
        }
    }

    // BSP planes are normally exact enough that this mostly removes float
    // noise. Keep distance quantisation finer than the strict planar validation
    // epsilon so unrelated parallel surfaces are not casually merged.
    const NORMAL_QUANT: f32 = 4096.0;
    const DISTANCE_QUANT: f32 = 4.0;
    Some(PlanarGroupKey {
        normal_x: (plane[0] * NORMAL_QUANT).round() as i32,
        normal_y: (plane[1] * NORMAL_QUANT).round() as i32,
        normal_z: (plane[2] * NORMAL_QUANT).round() as i32,
        distance: (plane[3] * DISTANCE_QUANT).round() as i32,
    })
}

pub(in crate::scene) fn planar_batch_plane(
    vertices: &[GpuVertex],
    strict: bool,
) -> Option<[f32; 4]> {
    let mut plane = None;
    for triangle in vertices.chunks_exact(3) {
        let a = Vec3::from_array(triangle[0].position);
        let b = Vec3::from_array(triangle[1].position);
        let c = Vec3::from_array(triangle[2].position);
        let normal = (b - a).cross(c - a).normalize_or_zero();
        if normal.length_squared() <= 1e-6 {
            continue;
        }
        plane = Some([normal.x, normal.y, normal.z, -normal.dot(a)]);
        break;
    }
    let plane = plane?;
    if !strict {
        return Some(plane);
    }

    let reference_normal = Vec3::new(plane[0], plane[1], plane[2]);
    if vertices.iter().any(|vertex| {
        (reference_normal.dot(Vec3::from_array(vertex.position)) + plane[3]).abs()
            > ENVIRONMENT_PLANAR_DISTANCE_EPSILON
    }) {
        return None;
    }
    for triangle in vertices.chunks_exact(3) {
        let a = Vec3::from_array(triangle[0].position);
        let b = Vec3::from_array(triangle[1].position);
        let c = Vec3::from_array(triangle[2].position);
        let normal = (b - a).cross(c - a).normalize_or_zero();
        if normal.length_squared() > 1e-6
            && normal.dot(reference_normal).abs() < ENVIRONMENT_PLANAR_NORMAL_DOT
        {
            return None;
        }
    }
    Some(plane)
}

pub(in crate::scene) fn assign_planar_reflection_planes(
    batches: &mut [DrawBatch],
    vertices: &[GpuVertex],
    environment: bool,
) {
    if !environment {
        for batch in batches.iter_mut() {
            batch.planar_environment_candidate = false;
        }
    }
    for batch in batches
        .iter_mut()
        .filter(|batch| batch.planar_reflection || batch.planar_environment_candidate)
    {
        let start = batch.vertices.start as usize;
        let end = batch.vertices.end as usize;
        let Some(slice) = vertices.get(start..end) else {
            batch.planar_environment_candidate = false;
            continue;
        };
        // Authored mirrors remain authoritative even if their tessellation is
        // slightly imperfect. Environment-map promotion is deliberately strict:
        // every triangle must describe the same geometric plane.
        let strict = batch.planar_environment_candidate && !batch.planar_reflection;
        if let Some(plane) = planar_batch_plane(slice, strict) {
            batch.planar_plane = plane;
        } else if batch.planar_reflection {
            batch.planar_reflection = false;
            batch.planar_environment_candidate = false;
        } else {
            batch.planar_environment_candidate = false;
        }
    }

    // Debug views need to survive multi-stage shaders. A tcGen environment
    // stage can be followed by opaque/alpha/lightmap stages that redraw the
    // exact same vertex range, which would otherwise hide a diagnostic color
    // emitted only by the environment stage. Propagate just the resolved plane
    // (not reflection eligibility) to sibling stages of the same surface. Normal
    // rendering still keys exclusively off planar_reflection / candidate flags.
    let resolved: std::collections::BTreeMap<(u32, u32), [f32; 4]> = batches
        .iter()
        .filter(|batch| {
            (batch.planar_reflection || batch.planar_environment_candidate)
                && Vec3::from_array([
                    batch.planar_plane[0],
                    batch.planar_plane[1],
                    batch.planar_plane[2],
                ])
                .length_squared()
                    > 0.5
        })
        .map(|batch| {
            (
                (batch.vertices.start, batch.vertices.end),
                batch.planar_plane,
            )
        })
        .collect();
    for batch in batches {
        if let Some(plane) = resolved.get(&(batch.vertices.start, batch.vertices.end)) {
            batch.planar_plane = *plane;
        }
    }
}

pub(in crate::scene) fn retain_authored_planar_mirrors(
    batches: &mut [DrawBatch],
    anchors: &[PortalSurfaceAnchor],
) {
    for batch in batches.iter_mut().filter(|batch| batch.planar_reflection) {
        let normal = Vec3::from_array([
            batch.planar_plane[0],
            batch.planar_plane[1],
            batch.planar_plane[2],
        ]);
        let distance = batch.planar_plane[3];
        let matching_anchor = anchors
            .iter()
            .filter_map(|anchor| {
                let plane_distance = (normal.dot(Vec3::from_array(anchor.origin)) + distance).abs();
                (plane_distance <= PORTAL_SURFACE_MAX_DISTANCE).then_some((plane_distance, *anchor))
            })
            .min_by(|(a, _), (b, _)| a.total_cmp(b));
        if let Some((_, anchor)) = matching_anchor.filter(|(_, anchor)| anchor.mirror) {
            batch.planar_pvs_origin = anchor.origin;
        } else {
            // A targeted misc_portal_surface denotes a camera portal, not a
            // mirror. Leave those authored portal surfaces on their material
            // path until the client has a real portal-camera entity renderer.
            batch.planar_reflection = false;
        }
    }
}

pub(in crate::scene) fn bsp_portal_surface_anchors(bsp: &Bsp) -> (Vec<PortalSurfaceAnchor>, usize) {
    let mut anchors = Vec::new();
    let mut camera_portals = 0usize;
    for entity in &bsp.entities {
        let Some(classname) = entity
            .get(b"classname")
            .and_then(|value| std::str::from_utf8(value).ok())
        else {
            continue;
        };
        if !classname.eq_ignore_ascii_case("misc_portal_surface") {
            continue;
        }
        let targeted = entity
            .get(b"target")
            .and_then(|value| std::str::from_utf8(value).ok())
            .is_some_and(|value| !value.trim().is_empty());
        if targeted {
            camera_portals += 1;
        }
        let Some(origin) = entity
            .get(b"origin")
            .and_then(|value| std::str::from_utf8(value).ok())
            .and_then(parse_light_triplet)
        else {
            continue;
        };
        anchors.push(PortalSurfaceAnchor {
            origin: render_position(origin),
            mirror: !targeted,
        });
    }
    (anchors, camera_portals)
}

pub(in crate::scene) fn map_portal_surface_anchors(
    document: &MapDocument,
) -> (Vec<PortalSurfaceAnchor>, usize) {
    let mut anchors = Vec::new();
    let mut camera_portals = 0usize;
    for entity in &document.entities {
        if !entity
            .classname()
            .is_some_and(|classname| classname.eq_ignore_ascii_case("misc_portal_surface"))
        {
            continue;
        }
        let targeted = entity
            .properties
            .get("target")
            .is_some_and(|target| !target.trim().is_empty());
        if targeted {
            camera_portals += 1;
        }
        let Some(origin) = entity
            .properties
            .get("origin")
            .and_then(|value| parse_light_triplet(value))
        else {
            continue;
        };
        anchors.push(PortalSurfaceAnchor {
            origin: render_position(origin),
            mirror: !targeted,
        });
    }
    (anchors, camera_portals)
}

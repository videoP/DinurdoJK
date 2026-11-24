//! Cold-path recognition of authored water, in BSP (Z-up) coordinates.
//! This deliberately does not change collision, shader batches or GPU state.
use jka_assets::{
    bsp::{Bsp, Mesh, SurfaceKind, SURF_NODRAW, SURF_SKY},
    shader::{Shader, TcGen, TcMod},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::{AtomicU8, Ordering},
};

pub(super) mod ownership;
pub(super) mod plan;

static MODE: AtomicU8 = AtomicU8::new(0);
pub fn mode() -> u8 {
    MODE.load(Ordering::Relaxed)
}
pub fn set_mode(mode: u8) {
    MODE.store(mode.min(3), Ordering::Relaxed);
}

const MAX_TRIANGLES: usize = 100_000;
const MAX_SURFACES: usize = 2048;

#[derive(Clone, Copy, Debug)]
pub(super) struct Triangle {
    pub points: [[f32; 3]; 3],
    /// Painted phase velocity, not a prescribed hydraulic velocity.
    pub flow: Option<[f32; 2]>,
    pub coherence: f32,
    pub alpha: [u8; 3],
}
#[derive(Clone, Debug)]
pub(super) struct Face {
    pub surface: usize,
    pub model: u32,
    pub shader: usize,
    pub triangles: Vec<Triangle>,
    /// Whole source surface can be replaced only if every triangle is a sheet.
    pub complete: bool,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub area: f32,
}

impl Face {
    fn is_fall(&self) -> bool {
        self.max[2] - self.min[2] > (self.max[0] - self.min[0]).max(self.max[1] - self.min[1])
    }
}

#[derive(Clone, Debug)]
pub(super) struct Body {
    pub faces: Vec<Face>,
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub area: f32,
    pub elongation: f32,
    pub coherence: f32,
    pub speed: f32,
    pub evidence: usize,
    pub candidate: bool,
}

fn vec3(bytes: &[u8]) -> Option<[f32; 3]> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut fields = text.split_ascii_whitespace();
    let p: [f32; 3] = [
        fields.next()?.parse().ok()?,
        fields.next()?.parse().ok()?,
        fields.next()?.parse().ok()?,
    ];
    (fields.next().is_none() && p.iter().all(|v| v.is_finite())).then_some(p)
}

/// Only uniquely placed, nonrotating func_static models are eligible. Animated
/// doors, movers, breakables and unknown entity poses must stay authored.
fn static_origins(bsp: &Bsp) -> BTreeMap<u32, [f32; 3]> {
    let mut origins = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut duplicates = BTreeSet::new();
    for entity in &bsp.entities {
        let Some(model) = entity
            .get(b"model")
            .and_then(|s| std::str::from_utf8(s).ok())
            .and_then(|s| s.strip_prefix('*'))
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if !seen.insert(model) {
            duplicates.insert(model);
        }
        if !entity
            .get(b"classname")
            .is_some_and(|s| s.eq_ignore_ascii_case(b"func_static"))
        {
            continue;
        }
        if entity
            .get(b"angles")
            .is_some_and(|s| vec3(s) != Some([0.0; 3]))
            || entity.get(b"angle").is_some_and(|s| {
                std::str::from_utf8(s)
                    .ok()
                    .and_then(|s| s.parse::<f32>().ok())
                    != Some(0.0)
            })
        {
            continue;
        }
        let origin = match entity.get(b"origin") {
            Some(s) => match vec3(s) {
                Some(p) => p,
                None => continue,
            },
            None => [0.0; 3],
        };
        origins.insert(model, origin);
    }
    origins.retain(|m, _| *m != 0 && !duplicates.contains(m));
    origins
}

fn stage_offsets(shader: Option<&Shader>) -> Vec<[f32; 2]> {
    shader
        .into_iter()
        .flat_map(|s| &s.stages)
        .filter(|s| matches!(s.tc_gen, TcGen::Base))
        .filter_map(|s| {
            // Rotate/stretch break a fixed heading; accept the other stages, not a
            // guessed constant transform. Turb is ignored by the ocean helper too.
            if s.tc_mods
                .iter()
                .any(|m| matches!(m, TcMod::Rotate(_) | TcMod::Stretch(_)))
            {
                return None;
            }
            crate::ocean::appearance::base_uv_scroll_velocity(&s.tc_mods)
        })
        .collect()
}

fn phase_velocity(
    points: [[f32; 3]; 3],
    uv: [[f32; 2]; 3],
    scrolls: &[[f32; 2]],
) -> (Option<[f32; 2]>, f32) {
    let a = [uv[1][0] - uv[0][0], uv[1][1] - uv[0][1]];
    let b = [uv[2][0] - uv[0][0], uv[2][1] - uv[0][1]];
    let det = a[0] * b[1] - a[1] * b[0];
    if det.abs() < 1e-8 {
        return (None, 0.0);
    }
    let mut v = [0.0; 2];
    let mut unit = [0.0; 2];
    let mut count = 0.0f32;
    for s in scrolls {
        let u = (s[0] * b[1] - s[1] * b[0]) / det;
        let w = (a[0] * s[1] - a[1] * s[0]) / det;
        let q: [f32; 2] = std::array::from_fn(|i| {
            (points[1][i] - points[0][i]) * u + (points[2][i] - points[0][i]) * w
        });
        let speed = q[0].hypot(q[1]);
        if !(0.01..=20_000.0).contains(&speed) {
            continue;
        }
        for i in 0..2 {
            v[i] += q[i];
            unit[i] += q[i] / speed;
        }
        count += 1.0;
    }
    if count == 0.0 {
        return (None, 0.0);
    }
    let coherence = unit[0].hypot(unit[1]) / count;
    ((coherence >= 0.65).then(|| v.map(|x| x / count)), coherence)
}

pub(super) fn area_xy(p: [[f32; 3]; 3]) -> f32 {
    ((p[1][0] - p[0][0]) * (p[2][1] - p[0][1]) - (p[2][0] - p[0][0]) * (p[1][1] - p[0][1])).abs()
        * 0.5
}

fn collect_faces(
    bsp: &Bsp,
    mesh: &Mesh,
    inline: &Mesh,
    inline_models: &[u32],
    library: &BTreeMap<String, Shader>,
) -> Result<Vec<Face>, String> {
    let origins = static_origins(bsp);
    let mut faces = Vec::new();
    let mut total = 0usize;
    for (mesh, models) in [(mesh, None), (inline, Some(inline_models))] {
        for (index, batch) in mesh.batches.iter().enumerate() {
            let model = models.and_then(|m| m.get(index)).copied().unwrap_or(0);
            let origin = if model == 0 {
                [0.0; 3]
            } else {
                let Some(p) = origins.get(&model) else {
                    continue;
                };
                *p
            };
            let bsp_shader = &bsp.shaders[batch.shader];
            let name = String::from_utf8_lossy(&bsp_shader.name).to_ascii_lowercase();
            let shader = jka_assets::shader::find_shader(library, &name);
            if bsp_shader.surface_flags & (SURF_NODRAW | SURF_SKY) != 0
                || shader.is_some_and(|s| s.nodraw || s.sky || s.lava || s.slime)
            {
                continue;
            }
            if bsp_shader.contents & jka_assets::bsp::bathymetry::CONTENTS_WATER == 0
                && !shader.is_some_and(|s| s.water)
            {
                continue;
            }
            if bsp.surfaces[batch.surface].kind == SurfaceKind::Flare {
                continue;
            }
            let scrolls = stage_offsets(shader);
            let mut face = Face {
                surface: batch.surface,
                model,
                shader: batch.shader,
                triangles: Vec::new(),
                complete: true,
                min: [f32::INFINITY; 3],
                max: [f32::NEG_INFINITY; 3],
                area: 0.0,
            };
            for ids in mesh.indices[batch.indices.clone()].chunks_exact(3) {
                let v = [0, 1, 2].map(|i| mesh.vertices[ids[i] as usize]);
                let points = v.map(|v| std::array::from_fn(|i| v.position[i] + origin[i]));
                let area = area_xy(points);
                let facing_up = v.iter().map(|v| v.normal[2]).sum::<f32>() >= -0.05;
                if !area.is_finite()
                    || !facing_up
                    || !points.iter().flatten().all(|v| v.is_finite())
                {
                    face.complete = false;
                    continue;
                }
                for p in points {
                    for i in 0..3 {
                        face.min[i] = face.min[i].min(p[i]);
                        face.max[i] = face.max[i].max(p[i]);
                    }
                }
                // Vertical water can mark a spill into a reach, but has no
                // footprint in a height field and must keep its authored draw.
                if area < 0.01 {
                    face.complete = false;
                    continue;
                }
                let (flow, coherence) = phase_velocity(points, v.map(|v| v.texcoord), &scrolls);
                face.triangles.push(Triangle {
                    points,
                    flow,
                    coherence,
                    alpha: v.map(|v| v.color[0][3]),
                });
                face.area += area;
                total += 1;
                if total > MAX_TRIANGLES {
                    return Err("water triangle budget exceeded".into());
                }
            }
            if face.area >= 64.0 || (face.is_fall() && face.max[2] - face.min[2] > 96.0) {
                faces.push(face);
            }
            if faces.len() > MAX_SURFACES {
                return Err("water surface budget exceeded".into());
            }
        }
    }
    Ok(faces)
}

fn shape(min: [f32; 3], max: [f32; 3], area: f32) -> f32 {
    let span = (max[0] - min[0]).max(max[1] - min[1]);
    span * span / area.max(1.0)
}
fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// Weld only actual shared 3D points (at least two per pair). Unlike bbox
/// overlap this cannot merge a river with an ocean under its mouth, or merge
/// two unrelated floors. A broad standing surface is kept out of narrow reach
/// components unless later hydraulic reconstruction explicitly couples it.
fn components(faces: Vec<Face>) -> Vec<Vec<Face>> {
    let mut points = BTreeMap::<[i64; 3], Vec<usize>>::new();
    for (i, f) in faces.iter().enumerate() {
        let keys: BTreeSet<_> = f
            .triangles
            .iter()
            .flat_map(|t| t.points)
            .map(|p| p.map(|v| (v as f64 * 4.0).round() as i64))
            .collect();
        for p in keys {
            points.entry(p).or_default().push(i);
        }
    }
    let mut shared = BTreeMap::<(usize, usize), u8>::new();
    let mut parent: Vec<_> = (0..faces.len()).collect();
    for ids in points.values() {
        // Nonmanifold junctions with many stacked decorative copies are unsafe.
        if ids.len() > 16 {
            continue;
        }
        for (k, &i) in ids.iter().enumerate() {
            for &j in &ids[k + 1..] {
                let a = &faces[i];
                let b = &faces[j];
                let broad = |f: &Face| {
                    shape(f.min, f.max, f.area) < 2.5
                        && f.area > 1_000_000.0
                        && f.max[2] - f.min[2] < 2.0
                };
                if broad(a) != broad(b) || a.is_fall() != b.is_fall() {
                    continue;
                }
                let count = shared.entry((i, j)).or_default();
                *count = count.saturating_add(1);
                if *count == 2 {
                    let x = root(&mut parent, i);
                    let y = root(&mut parent, j);
                    parent[x.max(y)] = x.min(y);
                }
            }
        }
    }
    let mut out = BTreeMap::<usize, Vec<Face>>::new();
    for (i, f) in faces.into_iter().enumerate() {
        out.entry(root(&mut parent, i)).or_default().push(f);
    }
    out.into_values().collect()
}

fn water_markers(bsp: &Bsp) -> Vec<[f32; 3]> {
    bsp.entities
        .iter()
        .filter_map(|e| {
            let hint = e.properties.iter().any(|(k, v)| {
                if ![
                    b"noise".as_slice(),
                    b"soundset",
                    b"fxFile",
                    b"effect",
                    b"sound",
                ]
                .iter()
                .any(|p| k.eq_ignore_ascii_case(p))
                {
                    return false;
                }
                let v = String::from_utf8_lossy(v).to_ascii_lowercase();
                ["waterfall", "water_", "river", "rapids", "stream"]
                    .iter()
                    .any(|s| v.contains(s))
            });
            hint.then(|| e.get(b"origin").and_then(vec3)).flatten()
        })
        .collect()
}

pub(super) fn detect(
    bsp: &Bsp,
    mesh: &Mesh,
    inline: &Mesh,
    inline_models: &[u32],
    library: &BTreeMap<String, Shader>,
) -> Result<Vec<Body>, String> {
    let markers = water_markers(bsp);
    let mut bodies = Vec::new();
    for faces in components(collect_faces(bsp, mesh, inline, inline_models, library)?) {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        let mut area = 0.0;
        let mut flow_area = 0.0;
        let mut coherence = 0.0;
        let mut speed = 0.0;
        for f in &faces {
            for i in 0..3 {
                min[i] = min[i].min(f.min[i]);
                max[i] = max[i].max(f.max[i]);
            }
            area += f.area;
            for t in &f.triangles {
                let a = area_xy(t.points);
                if let Some(v) = t.flow {
                    flow_area += a;
                    speed += a * v[0].hypot(v[1]);
                    coherence += a * t.coherence;
                }
            }
        }
        let elongation = shape(min, max, area);
        let coherence = coherence / area.max(1.0);
        let speed = speed / flow_area.max(1.0);
        let evidence = markers
            .iter()
            .filter(|p| {
                faces
                    .iter()
                    .any(|f| (0..3).all(|i| p[i] >= f.min[i] - 128.0 && p[i] <= f.max[i] + 128.0))
            })
            .count();
        let extent = (max[0] - min[0]).max(max[1] - min[1]);
        let drop = max[2] - min[2];
        // A geometry prior plus at least one independent motion/elevation/FX
        // signal. Names are reported, never sufficient to select a river.
        let candidate = drop <= extent
            && extent >= 384.0
            && area >= 8192.0
            && ((elongation >= 3.5
                && (coherence >= 0.55 && speed >= 12.0 || drop >= 32.0 || evidence >= 3))
                || (elongation >= 2.0 && drop >= 128.0 && coherence >= 0.55));
        bodies.push(Body {
            faces,
            min,
            max,
            area,
            elongation,
            coherence,
            speed,
            evidence,
            candidate,
        });
    }
    Ok(bodies)
}

pub(super) fn report(bsp: &Bsp, bodies: &[Body]) {
    for (i, b) in bodies.iter().enumerate() {
        let ids: Vec<_> = b
            .faces
            .iter()
            .map(|f| {
                (
                    f.surface,
                    f.model,
                    String::from_utf8_lossy(&bsp.shaders[f.shader].name),
                )
            })
            .collect();
        let classification = if b.candidate {
            "river candidate"
        } else if b.faces.iter().all(Face::is_fall) {
            "waterfall"
        } else {
            "authored water"
        };
        eprintln!(
            "[RIVER PROMOTION] body={i} {classification} surfaces={ids:?} bounds={:?}..{:?} area={:.0} shape={:.2} scroll_coherence={:.2} phase_speed={:.1} markers={}",
            b.min, b.max, b.area, b.elongation, b.coherence, b.speed, b.evidence,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn visible_phase_uses_uv_scale_and_rejects_opposed_stages() {
        let points = [[0., 0., 0.], [100., 0., 0.], [0., 50., 0.]];
        let uv = [[0., 0.], [1., 0.], [0., 1.]];
        assert_eq!(phase_velocity(points, uv, &[[2., 0.]]).0, Some([200., 0.]));
        assert!(phase_velocity(points, uv, &[[2., 0.], [-1., 0.]])
            .0
            .is_none());
    }
}

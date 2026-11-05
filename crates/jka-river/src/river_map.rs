//! Map authoring adapter, separate from the authoritative Unreal solver.
//! Brush markers supply endpoint sections/headings/heights. They do not supply
//! an Unreal WaterBody spline, nor silently turn a discharge into a velocity.
use jka_assets::bsp::{
    river::{Inlet, Outlet, River, Volume},
    Bsp,
};
use jka_water::river::{
    authoring::{Domain, Endpoint, SOURCE_SIZE_CM},
    boundary::SourceBox,
    CENTIMETRES_PER_MAP_UNIT,
};

pub const CUBIC_CENTIMETRES_PER_CUBIC_FOOT: f64 = 28316.846592;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RequestedInlet {
    DischargeCm3PerSecond(f64),
    VelocityCmPerSecond(f64),
}

pub struct MapSource {
    pub entity_index: usize,
    pub endpoint: Endpoint,
    /// Actual disjoint transverse intersections, in map units relative to the
    /// marker centre. The native endpoint uses their outer span as its width.
    pub sections_map: Vec<[f64; 2]>,
    pub requested: RequestedInlet,
}
pub struct MapSink {
    pub entity_index: usize,
    pub endpoint: Endpoint,
    pub sections_map: Vec<[f64; 2]>,
    pub requested: Outlet,
}
pub struct MapRiver {
    pub name: String,
    pub domain: Domain,
    pub sources: Vec<MapSource>,
    pub sinks: Vec<MapSink>,
}

impl MapRiver {
    /// Resolution is an explicit native max-axis setting, not JAPRO cellSize.
    /// Map friction/drainrate remain metadata; they are not native coefficients.
    pub fn from_bsp(bsp: &Bsp, river: &River, maximum_axis: u32) -> Result<Self, String> {
        let scale = f64::from(CENTIMETRES_PER_MAP_UNIT);
        let domain = Domain::from_bounds(
            river.volume.mins.map(|v| f64::from(v) * scale),
            river.volume.maxs.map(|v| f64::from(v) * scale),
            maximum_axis,
        )?;
        let sources = river
            .sources
            .iter()
            .map(|s| {
                let (endpoint, sections_map) =
                    endpoint(bsp, &s.volume, s.angle_degrees, s.surface_height)
                        .map_err(|e| format!("river source entity {}: {e}", s.entity_index))?;
                Ok(MapSource {
                    entity_index: s.entity_index,
                    endpoint,
                    sections_map,
                    requested: match s.inlet {
                        Inlet::DischargeCfs(q) => RequestedInlet::DischargeCm3PerSecond(
                            f64::from(q) * CUBIC_CENTIMETRES_PER_CUBIC_FOOT,
                        ),
                        Inlet::Velocity(v) => {
                            RequestedInlet::VelocityCmPerSecond(f64::from(v) * scale)
                        }
                    },
                })
            })
            .collect::<Result<_, String>>()?;
        let sinks = river
            .sinks
            .iter()
            .map(|s| {
                let (endpoint, sections_map) =
                    endpoint(bsp, &s.volume, s.angle_degrees, s.surface_height)
                        .map_err(|e| format!("river sink entity {}: {e}", s.entity_index))?;
                Ok(MapSink {
                    entity_index: s.entity_index,
                    endpoint,
                    sections_map,
                    requested: s.outlet,
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            name: river.name.clone(),
            domain,
            sources,
            sinks,
        })
    }

    /// Only the selected native HEIGHT-controlled boundary operations. This
    /// explicitly does not enforce the requested discharge/velocity or map drain
    /// rate. Acceptance/bake code must not label this a discharge-matched solve.
    pub fn height_boundary_boxes(&self) -> Result<Vec<SourceBox>, String> {
        let mut boxes = Vec::new();
        for s in &self.sources {
            boxes.push(self.domain.boundary(s.endpoint, false, SOURCE_SIZE_CM)?);
        }
        for s in &self.sinks {
            boxes.push(self.domain.boundary(s.endpoint, true, SOURCE_SIZE_CM)?);
        }
        Ok(boxes)
    }
}

/// Exact horizontal line/convex-brush clipping through the marker's centre,
/// transverse to its heading. Retain separate sections, then author one native
/// endpoint at the midpoint of their outer span. Unreal's endpoint width is a
/// scalar, not a brush mask; the gaps remain explicit adapter metadata.
fn endpoint(
    bsp: &Bsp,
    volume: &Volume,
    heading: f32,
    surface_height: f32,
) -> Result<(Endpoint, Vec<[f64; 2]>), String> {
    if !heading.is_finite() || !surface_height.is_finite() {
        return Err("invalid marker heading/height".into());
    }
    let angle = f64::from(heading).to_radians();
    let (sine, cosine) = angle.sin_cos();
    let transverse = [-sine, cosine, 0.0];
    let centre = std::array::from_fn::<_, 3, _>(|i| {
        (f64::from(volume.mins[i]) + f64::from(volume.maxs[i])) * 0.5
    });
    let local = std::array::from_fn::<_, 3, _>(|i| centre[i] - f64::from(volume.origin[i]));
    let model = bsp
        .models
        .get(volume.model)
        .ok_or("invalid river marker model")?;
    let brushes = bsp
        .brushes
        .get(model.brushes.clone())
        .ok_or("invalid river marker brush range")?;
    let sections = section(&bsp.planes, &bsp.brush_sides, brushes, local, transverse)?;
    let span = [sections[0][0], sections.last().unwrap()[1]];
    let scale = f64::from(CENTIMETRES_PER_MAP_UNIT);
    let middle = (span[0] + span[1]) * 0.5;
    let position = [
        (centre[0] + middle * transverse[0]) * scale,
        (centre[1] + middle * transverse[1]) * scale,
        f64::from(surface_height) * scale,
    ];
    let width = ((span[1] - span[0]) * scale) as f32;
    if !width.is_finite() || width <= 0.0 || position.iter().any(|v| !v.is_finite()) {
        return Err("river marker endpoint overflow".into());
    }
    Ok((
        Endpoint {
            position,
            tangent: [cosine, sine, 0.0],
            width,
        },
        sections,
    ))
}

fn section(
    planes: &[jka_assets::bsp::Plane],
    sides: &[jka_assets::bsp::BrushSide],
    brushes: &[jka_assets::bsp::Brush],
    local: [f64; 3],
    transverse: [f64; 3],
) -> Result<Vec<[f64; 2]>, String> {
    let mut intervals = Vec::new();
    for brush in brushes {
        let mut low = f64::NEG_INFINITY;
        let mut high = f64::INFINITY;
        let mut empty = false;
        for side in sides
            .get(brush.sides.clone())
            .ok_or("invalid river marker sides")?
        {
            let plane = planes.get(side.plane).ok_or("invalid river marker plane")?;
            let n = plane.normal.map(f64::from);
            if !plane.distance.is_finite() || n.iter().any(|v| !v.is_finite()) {
                return Err("invalid river marker plane values".into());
            }
            let denominator = n[0] * transverse[0] + n[1] * transverse[1];
            let numerator =
                f64::from(plane.distance) - (n[0] * local[0] + n[1] * local[1] + n[2] * local[2]);
            if denominator == 0.0 {
                if numerator < 0.0 {
                    empty = true;
                    break;
                }
            } else if denominator > 0.0 {
                high = high.min(numerator / denominator);
            } else {
                low = low.max(numerator / denominator);
            }
        }
        if !empty && high > low {
            if !low.is_finite() || !high.is_finite() {
                return Err("unbounded river marker cross-section".into());
            }
            intervals.push([low, high]);
        }
    }
    intervals.sort_by(|a, b| a[0].total_cmp(&b[0]));
    let mut joined: Vec<[f64; 2]> = Vec::new();
    for interval in intervals {
        if let Some(last) = joined.last_mut().filter(|v| interval[0] <= v[1]) {
            last[1] = last[1].max(interval[1]);
        } else {
            joined.push(interval);
        }
    }
    if joined.is_empty() {
        return Err("river marker has no transverse section at its centre height".into());
    }
    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn marker_width_clips_brush_planes_instead_of_projecting_the_aabb() {
        use jka_assets::bsp::{Brush, BrushSide, Plane};
        let planes: Vec<_> = [
            ([1.0, 0.0, 0.0], 2.0),
            ([-1.0, 0.0, 0.0], 2.0),
            ([0.0, 1.0, 0.0], 1.0),
            ([0.0, -1.0, 0.0], 1.0),
            ([0.0, 0.0, 1.0], 2.0),
            ([0.0, 0.0, -1.0], 2.0),
        ]
        .into_iter()
        .map(|(normal, distance)| Plane { normal, distance })
        .collect();
        let sides: Vec<_> = (0..6).map(|plane| BrushSide { plane, shader: 0 }).collect();
        let brush = Brush {
            sides: 0..6,
            shader: 0,
        };
        let s = std::f64::consts::FRAC_1_SQRT_2;
        let result = section(&planes, &sides, &[brush], [0.0; 3], [-s, s, 0.0]).unwrap();
        assert_eq!(result.len(), 1);
        let result = result[0];
        assert!(((result[1] - result[0]) - 2.0 * 2.0f64.sqrt()).abs() < 1e-12);
        let projection = 6.0 * s;
        assert!((projection - (result[1] - result[0])).abs() > 1.0);
    }

    #[test]
    fn disconnected_sections_retain_the_gap_and_union_overlapping_brushes() {
        use jka_assets::bsp::{Brush, BrushSide, Plane};
        let mut planes = Vec::new();
        let mut sides = Vec::new();
        let mut brushes = Vec::new();
        for [low, high] in [[-4.0, -2.0], [1.0, 3.0], [2.0, 5.0]] {
            let first = sides.len();
            for (normal, distance) in [
                ([1.0, 0.0, 0.0], 1.0),
                ([-1.0, 0.0, 0.0], 1.0),
                ([0.0, 1.0, 0.0], high),
                ([0.0, -1.0, 0.0], -low),
                ([0.0, 0.0, 1.0], 1.0),
                ([0.0, 0.0, -1.0], 1.0),
            ] {
                sides.push(BrushSide {
                    plane: planes.len(),
                    shader: 0,
                });
                planes.push(Plane { normal, distance });
            }
            brushes.push(Brush {
                sides: first..sides.len(),
                shader: 0,
            });
        }
        let result = section(&planes, &sides, &brushes, [0.0; 3], [0.0, 1.0, 0.0]).unwrap();
        assert_eq!(result, vec![[-4.0, -2.0], [1.0, 5.0]]);
        assert_eq!(result.last().unwrap()[1] - result[0][0], 9.0);
    }
}

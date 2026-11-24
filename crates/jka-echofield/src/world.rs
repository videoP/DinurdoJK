//! Static BSP adapter. Material equivalences are explicitly a JKA policy,
//! not attributed to upstream. No full-map voxel grid and no per-ray brush scan.
use crate::{
    geometry::{
        from_upstream, to_upstream, Mover, Movers, SolidInterval, SolidTraceScratch, SurfaceBvh,
        SurfaceHit, Vec3,
    },
    profile::{
        add, dot, golden_direction, length, scale, sub, AcousticWorld, Hit, Material,
        SpectralCategory as S,
    },
    terrain::{compute_saliency, FeatureKind, TerrainFeature},
};
use jka_assets::bsp::{
    AcousticCollision, AcousticMesh, ACOUSTIC_SKY, CONTENTS_SOLID, CONTENTS_TERRAIN,
};
use std::sync::Arc;

const WATER: u32 = jka_assets::bsp::bathymetry::CONTENTS_WATER;
const LAVA: u32 = 2;

#[derive(Clone, Copy, Debug)]
pub struct Contact {
    pub position: Vec3,
    pub normal: Vec3,
    pub material: u8,
    pub contents: u32,
    /// Water already owned by the game's hydraulic sound layer.
    pub river: bool,
}

impl Contact {
    pub fn river(origin: [f32; 3]) -> Self {
        Self {
            position: to_upstream(origin.map(f64::from)),
            normal: [0.0, 1.0, 0.0],
            material: 0,
            contents: WATER,
            river: true,
        }
    }
    pub fn water(self) -> bool {
        self.contents & WATER != 0
    }
    pub fn lava(self) -> bool {
        self.contents & LAVA != 0
    }
}
/// JKA surfaceflags.h -> closest named SPR material. Coefficients are from
/// ReflectivityConfig and OcclusionConfig at 4a595e7. Override per map if needed.
pub fn material(id: u8) -> Material {
    let (spectral, reflectivity, occlusion) = match id {
        1 | 2 => (S::Wood, 0.4, 1.),              // wood
        3 | 4 | 30 | 31 => (S::Hard, 1.25, 1.),   // metal
        5 | 6 | 19 | 20 => (S::Foliage, 0.3, 1.), // grass/leaves
        8 => (S::Soft, 0.2, 1.),
        9 => (S::Soft, 0.3, 1.),
        10 | 18 | 29 => (S::Hard, 0.75, 0.1),         // glass
        11 | 12 | 23 | 26 | 28 => (S::Hard, 1.5, 1.), // stone
        13 => (S::Default, 0.5, 0.25),
        14 => (S::Soft, 0.15, 0.1),
        21 | 22 | 27 => (S::Soft, 0.1, 1.5), // wool analogy
        17 => (S::Soft, 0.5, 1.),
        15 => (S::Hard, 0.5, 1.),
        _ => (S::Default, 0.5, 1.),
    };
    Material {
        spectral,
        reflectivity,
        occlusion,
    }
}
pub struct BspWorld {
    bsp: Arc<AcousticCollision>,
    surfaces: SurfaceBvh,
    solid: SolidTraceScratch,
    movers: Movers,
    moving_intervals: Vec<SolidInterval>,
    events: Vec<(f64, bool, u8)>,
    materials: [Material; 32],
}
impl BspWorld {
    pub fn build(
        bsp: Arc<AcousticCollision>,
        mesh: Arc<AcousticMesh>,
    ) -> Result<Self, &'static str> {
        Self::build_cancellable(bsp, mesh, &|| false)
    }
    pub fn build_cancellable(
        bsp: Arc<AcousticCollision>,
        mesh: Arc<AcousticMesh>,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Self, &'static str> {
        Ok(Self {
            bsp,
            surfaces: SurfaceBvh::build_cancellable(mesh, cancelled)?,
            solid: SolidTraceScratch::default(),
            movers: Movers::default(),
            moving_intervals: Vec::new(),
            events: Vec::new(),
            materials: std::array::from_fn(|i| material(i as u8)),
        })
    }
    pub fn set_material(&mut self, id: u8, value: Material) {
        if let Some(m) = self.materials.get_mut(id as usize) {
            *m = value;
        }
    }

    pub fn set_movers(&mut self, movers: &[Mover]) {
        self.movers.update(&self.bsp, movers);
    }

    pub fn dynamic_occlusion(&mut self, from: Vec3, to: Vec3) -> f64 {
        let distance = length(sub(to, from));
        if distance <= 0.0 {
            return 0.0;
        }
        self.events.clear();
        for (hit, _) in self.movers.hits(
            &self.bsp,
            from_upstream(from),
            from_upstream(to),
            CONTENTS_SOLID | CONTENTS_TERRAIN,
        ) {
            self.events.push((hit.enter, true, hit.material & 31));
            self.events.push((hit.exit, false, hit.material & 31));
        }
        interval_cost(&mut self.events, &self.materials, distance, 64.0, 16).0
    }

    pub fn submerged(&mut self, position: Vec3) -> bool {
        let from = from_upstream(position);
        let to = add(from, [0.0, 0.0, 0.01]);
        self.solid
            .trace_contents(&self.bsp, from, to, WATER | LAVA)
            .is_ok_and(|hits| hits.iter().any(|hit| hit.enter == 0.0))
            || self
                .movers
                .hits(&self.bsp, from, to, WATER | LAVA)
                .any(|(hit, _)| hit.enter == 0.0)
    }

    fn nearest_surface(&self, from: Vec3, to: Vec3, minimum: f64) -> Option<SurfaceHit> {
        let surface = self.surfaces.nearest(from, to, minimum);
        let max = surface.as_ref().map_or(1.0, |hit| hit.fraction);
        let moving = self
            .movers
            .hits(&self.bsp, from, to, CONTENTS_SOLID | CONTENTS_TERRAIN)
            .filter_map(|(hit, normal)| {
                let normal = normal?;
                (hit.enter >= minimum && hit.enter < max).then_some((hit, normal))
            })
            .min_by(|(a, _), (b, _)| a.enter.total_cmp(&b.enter));
        moving
            .map(|(hit, normal)| SurfaceHit {
                fraction: hit.enter,
                position: add(from, scale(sub(to, from), hit.enter)),
                normal,
                material: hit.material,
                triangle: u32::MAX,
                flags: 0,
            })
            .or(surface)
    }

    /// Sample nearby exposed surfaces and liquid boundaries for the ambience
    /// scanner. The BVH keeps probes proportional to local geometry.
    pub fn ambience_contacts(&mut self, listener: Vec3) -> Vec<Contact> {
        const RAYS: usize = 96;
        const RANGE: f64 = 32.0;
        let from = from_upstream(listener);
        let mut contacts = Vec::with_capacity(RAYS);
        for i in 0..RAYS {
            let direction = golden_direction(i, RAYS, true);
            let to = from_upstream(add(listener, scale(direction, RANGE)));
            let surface = self.nearest_surface(from, to, 0.0001);
            let limit = surface.as_ref().map_or(1.0, |hit| hit.fraction);
            let static_liquid = self
                .solid
                .trace_contents(&self.bsp, from, to, WATER | LAVA)
                .ok()
                .and_then(|hits| hits.first().copied());
            let moving_liquids = self
                .movers
                .hits(&self.bsp, from, to, WATER | LAVA)
                .map(|(hit, _)| hit);
            if let Some(hit) = static_liquid
                .into_iter()
                .chain(moving_liquids)
                .filter(|hit| hit.enter <= limit)
                .min_by(|a, b| a.enter.total_cmp(&b.enter))
            {
                let contents = self.bsp.shaders[self.bsp.brushes[hit.brush].shader].contents;
                contacts.push(Contact {
                    position: add(listener, scale(direction, RANGE * hit.enter)),
                    normal: [0.0, 1.0, 0.0],
                    material: hit.material,
                    contents,
                    river: false,
                });
                continue;
            }
            if let Some(hit) = surface.filter(|hit| hit.flags & ACOUSTIC_SKY == 0) {
                let [x, y, z] = hit.normal;
                let mut normal = [x, z, -y];
                if dot(normal, direction) > 0.0 {
                    normal = scale(normal, -1.0);
                }
                contacts.push(Contact {
                    position: to_upstream(hit.position),
                    normal,
                    material: hit.material,
                    contents: 0,
                    river: false,
                });
            }
        }
        contacts
    }

    /// Detect height transitions and enclosed features in the same local space
    /// as the emitter scan. The probes use the actual surface on either side
    /// of each contact instead of assigning terrain types by material alone.
    pub fn terrain_features(
        &mut self,
        listener: Vec3,
        contacts: &[Contact],
        radius: f64,
    ) -> Vec<TerrainFeature<S>> {
        let mut features = Vec::new();
        let mut walls = Vec::new();
        for contact in contacts
            .iter()
            .filter(|contact| !contact.water() && !contact.lava())
        {
            let origin = contact.position;
            let normal = contact.normal;
            let spectral = self.materials[contact.material as usize & 31].spectral;
            let distance = length(sub(listener, origin));
            if distance > radius {
                continue;
            }
            if normal[1] > 0.65 {
                for outward in [[1., 0., 0.], [-1., 0., 0.], [0., 0., 1.], [0., 0., -1.]] {
                    let adjacent = add(origin, outward);
                    let start = add(adjacent, [0.0, 1.75, 0.0]);
                    let end = add(adjacent, [0.0, -16.0, 0.0]);
                    let Some(floor) = self.cast(start, end).filter(|floor| floor.normal[1] > 0.5)
                    else {
                        push_feature(
                            &mut features,
                            listener,
                            FeatureKind::Edge,
                            origin,
                            outward,
                            8.0,
                            spectral,
                        );
                        continue;
                    };
                    let height = (origin[1] - floor.position[1]) as f32;
                    if height >= 2.0 {
                        push_feature(
                            &mut features,
                            listener,
                            FeatureKind::Edge,
                            origin,
                            outward,
                            height,
                            spectral,
                        );
                        if height >= 4.0 {
                            push_feature(
                                &mut features,
                                listener,
                                FeatureKind::Drop,
                                floor.position,
                                floor.normal,
                                height,
                                floor.material.spectral,
                            );
                        }
                    } else if height.abs() >= 0.20 {
                        let step = if height >= 0.0 {
                            floor.position
                        } else {
                            origin
                        };
                        push_feature(
                            &mut features,
                            listener,
                            FeatureKind::Step,
                            step,
                            outward,
                            height.abs(),
                            spectral,
                        );
                    }
                }
            } else if normal[1] < -0.65 {
                let height = (origin[1] - listener[1]) as f32;
                if (1.0..=16.0).contains(&height) {
                    push_feature(
                        &mut features,
                        listener,
                        FeatureKind::Ceiling,
                        origin,
                        normal,
                        height,
                        spectral,
                    );
                }
            } else if normal[1].abs() < 0.35 {
                let tangent = [normal[2], 0.0, -normal[0]];
                let outside = add(origin, scale(normal, 0.15));
                let side_blocked = |world: &mut Self, sign: f64| {
                    let start = add(outside, scale(tangent, sign * 1.5));
                    world.cast(start, add(start, scale(normal, -0.5))).is_some()
                };
                let kind = if !side_blocked(self, -1.0) && !side_blocked(self, 1.0) {
                    FeatureKind::SolidObject
                } else {
                    FeatureKind::Wall
                };
                push_feature(&mut features, listener, kind, origin, normal, 2.0, spectral);
                walls.push((origin, normal));
            }
        }

        for (i, &(left, inward)) in walls.iter().enumerate() {
            let right = walls[i + 1..].iter().find_map(|&(right, other)| {
                let delta = sub(right, left);
                let width = dot(delta, inward);
                let lateral2 = (dot(delta, delta) - width * width).max(0.0);
                (dot(inward, other) < -0.8 && (1.0..=8.0).contains(&width) && lateral2 <= 4.0)
                    .then_some((right, width))
            });
            if let Some((right, width)) = right {
                let midpoint = scale(add(left, right), 0.5);
                let along = [inward[2], 0.0, -inward[0]];
                push_feature(
                    &mut features,
                    listener,
                    FeatureKind::Passage,
                    midpoint,
                    along,
                    width as f32,
                    S::Hard,
                );
            }
        }
        features
    }
}

fn push_feature(
    out: &mut Vec<TerrainFeature<S>>,
    listener: Vec3,
    kind: FeatureKind,
    position: Vec3,
    normal: Vec3,
    magnitude: f32,
    material: S,
) {
    let delta = sub(listener, position);
    let distance = length(delta);
    if distance >= f64::from(kind.max_range()) {
        return;
    }
    let direction = scale(delta, 1.0 / distance.max(0.001));
    let saliency = compute_saliency(
        magnitude,
        16.0,
        distance as f32,
        kind.max_range(),
        normal.map(|v| v as f32),
        direction.map(|v| v as f32),
        1.0,
    );
    if saliency > 0.0 {
        out.push(TerrainFeature {
            kind,
            position: position.map(|v| v as f32),
            normal: normal.map(|v| v as f32),
            magnitude,
            saliency,
            material,
        });
    }
}
impl AcousticWorld for BspWorld {
    fn cast(&mut self, from: Vec3, to: Vec3) -> Option<Hit> {
        let d = length(sub(to, from));
        if d < 1e-6 {
            return None;
        }
        let hit =
            self.nearest_surface(from_upstream(from), from_upstream(to), (1e-5 / d).min(1.))?;
        if hit.flags & ACOUSTIC_SKY != 0 {
            return None;
        } // sky is an escape, not a reflecting ceiling
        let [x, y, z] = hit.normal;
        let mut normal = [x, z, -y];
        if dot(normal, sub(to, from)) > 0. {
            normal = normal.map(|v| -v);
        }
        Some(Hit {
            position: to_upstream(hit.position),
            normal,
            material: self.materials[hit.material as usize & 31],
        })
    }
    fn occlusion(&mut self, from: Vec3, to: Vec3, max: f64, max_steps: usize) -> f64 {
        let distance = length(sub(to, from));
        if distance <= 0. || max_steps == 0 {
            return 0.;
        }
        let Ok(intervals) = self
            .solid
            .trace(&self.bsp, from_upstream(from), from_upstream(to))
        else {
            return 0.;
        };
        self.events.clear();
        for i in intervals {
            let m = i.material & 31;
            self.events.push((i.enter, true, m));
            self.events.push((i.exit, false, m));
        }
        self.moving_intervals.clear();
        self.moving_intervals.extend(
            self.movers
                .hits(
                    &self.bsp,
                    from_upstream(from),
                    from_upstream(to),
                    CONTENTS_SOLID | CONTENTS_TERRAIN,
                )
                .map(|(hit, _)| hit),
        );
        for hit in &self.moving_intervals {
            self.events.push((hit.enter, true, hit.material & 31));
            self.events.push((hit.exit, false, hit.material & 31));
        }
        let (mut result, mut traversed) =
            interval_cost(&mut self.events, &self.materials, distance, max, max_steps);
        if result >= max || traversed >= max_steps as f64 {
            return result;
        }
        // BSP patch/triangle surfaces have no reliable authored thickness.
        // Map one uncovered crossing to the upstream non-full-block
        // occlusion factor (0.25), consuming one equivalent block step.
        // This is a host mapping, not a physical thickness measurement.
        let start = from_upstream(from);
        let end = from_upstream(to);
        let mut min = (1e-5 / distance).min(1.);
        for _ in 0..max_steps {
            let Some(hit) = self.surfaces.nearest_thin(start, end, min) else {
                break;
            };
            min =
                hit.fraction + (0.001 / crate::geometry::JKA_UNITS_PER_METER / distance).max(1e-7);
            if !intervals
                .iter()
                .chain(&self.moving_intervals)
                .any(|i| hit.fraction >= i.enter - 1e-6 && hit.fraction <= i.exit + 1e-6)
            {
                let l = 1.0_f64.min(max_steps as f64 - traversed);
                result +=
                    0.25 * l * f64::from(self.materials[hit.material as usize & 31].occlusion);
                traversed += l;
                if result >= max || traversed >= max_steps as f64 {
                    break;
                }
            }
            if min >= 1. {
                break;
            }
        }
        result.min(max)
    }
}

fn interval_cost(
    events: &mut [(f64, bool, u8)],
    materials: &[Material; 32],
    distance: f64,
    max: f64,
    max_steps: usize,
) -> (f64, f64) {
    events.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    // Adaptation: a metre of solid is one equivalent upstream block. For
    // overlapping solids, choose the highest active resistance; never sum
    // coincident brushes. Limit total traversed solid length to the upstream
    // block-step budget, not the number of BSP brush records.
    let mut active = [0_u32; 32];
    let mut last = 0.;
    let mut traversed = 0.;
    let mut result = 0.;
    for &(t, enter, m) in events.iter() {
        if active.iter().any(|&n| n > 0) {
            let l = ((t - last) * distance).min(max_steps as f64 - traversed);
            let resistance = active
                .iter()
                .enumerate()
                .filter(|(_, n)| **n > 0)
                .map(|(i, _)| f64::from(materials[i].occlusion))
                .fold(0., f64::max);
            result += l * resistance;
            traversed += l;
            if result >= max || traversed >= max_steps as f64 {
                return (result.min(max), traversed);
            }
        }
        if enter {
            active[m as usize] += 1;
        } else {
            active[m as usize] = active[m as usize].saturating_sub(1);
        }
        last = t;
    }
    (result, traversed)
}

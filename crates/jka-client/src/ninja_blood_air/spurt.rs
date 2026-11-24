use crate::{
    fx::system::{FxDraw, FxFrame},
    renderer::DynamicModelVertex,
};
use glam::{Quat, Vec3};
use jka_assets::pk3::AssetSearchPath;
use std::{num::NonZeroU32, path::Path, sync::Arc};

const MESH_ASSET: &str = "effects/dinurdo/blood/impact.djkspurt";
const MAX_MESH_BYTES: usize = 8 * 1024 * 1024;
const MAX_TRIANGLES: usize = 25_000;
const MAX_SPURTS: usize = 3;
const MAP_UNITS_PER_SOURCE_UNIT: f32 = 3.0;
const IMPACT_HOLD_MS: i32 = 17;

struct MeshFrame {
    vertices: Vec<(Vec3, Vec3)>,
    indices: Arc<Vec<u32>>,
    anchor: Vec3,
}

struct Clip {
    frames: Vec<MeshFrame>,
    fps_numerator: u16,
    fps_denominator: u16,
    last_visible_frame: usize,
}

/// `<[u8]>::split_off_first_chunk` (Rust 1.87) for the pinned toolchain.
fn take_chunk<'a, const N: usize>(bytes: &mut &'a [u8]) -> Option<&'a [u8; N]> {
    let (head, rest) = bytes.split_first_chunk::<N>()?;
    *bytes = rest;
    Some(head)
}

fn read_vec3(bytes: &mut &[u8]) -> Option<Vec3> {
    let value = Vec3::from_array([
        f32::from_le_bytes(*take_chunk::<4>(bytes)?),
        f32::from_le_bytes(*take_chunk::<4>(bytes)?),
        f32::from_le_bytes(*take_chunk::<4>(bytes)?),
    ]);
    value.is_finite().then_some(value)
}

fn read_u16(bytes: &mut &[u8]) -> Option<u16> {
    Some(u16::from_le_bytes(*take_chunk::<2>(bytes)?))
}

fn decode_normal([x, y]: [u8; 2]) -> Vec3 {
    let (x, y) = (f32::from(x) / 127.5 - 1.0, f32::from(y) / 127.5 - 1.0);
    let z = 1.0 - x.abs() - y.abs();
    if z < 0.0 {
        Vec3::new(
            (1.0 - y.abs()) * x.signum(),
            (1.0 - x.abs()) * y.signum(),
            z,
        )
    } else {
        Vec3::new(x, y, z)
    }
    .normalize()
}

impl Clip {
    fn decode(mut bytes: &[u8]) -> Option<Self> {
        if bytes.len() > MAX_MESH_BYTES || take_chunk::<8>(&mut bytes)? != b"DJKSPRT1" {
            return None;
        }
        let fps_numerator = read_u16(&mut bytes)?;
        let fps_denominator = read_u16(&mut bytes)?;
        let frame_count = usize::from(read_u16(&mut bytes)?);
        let triangle_limit = usize::from(read_u16(&mut bytes)?);
        let fps = f32::from(fps_numerator) / f32::from(fps_denominator);
        if !(1.0..=120.0).contains(&fps)
            || !(1..=120).contains(&frame_count)
            || !(1..=MAX_TRIANGLES).contains(&triangle_limit)
        {
            return None;
        }

        let mut frames = Vec::with_capacity(frame_count);
        let mut surface_areas = Vec::with_capacity(frame_count);
        for _ in 0..frame_count {
            let vertex_count = usize::from(read_u16(&mut bytes)?);
            let triangles = usize::from(read_u16(&mut bytes)?);
            let lo = read_vec3(&mut bytes)?;
            let hi = read_vec3(&mut bytes)?;
            let span = hi - lo;
            if triangles == 0
                || triangles > triangle_limit
                || !(3..=triangles * 3).contains(&vertex_count)
                || span.min_element() < 0.0
                || lo.abs().max(hi.abs()).max_element() > 128.0
                || bytes.len() < 8 * vertex_count + 6 * triangles
            {
                return None;
            }
            let mut vertices = Vec::with_capacity(vertex_count);
            let mut center = Vec3::ZERO;
            for _ in 0..vertex_count {
                let point = Vec3::new(
                    f32::from(read_u16(&mut bytes)?),
                    f32::from(read_u16(&mut bytes)?),
                    f32::from(read_u16(&mut bytes)?),
                );
                let normal = decode_normal(*take_chunk::<2>(&mut bytes)?);
                let position = lo + span * (point / 65535.0);
                center += position;
                vertices.push((position, normal));
            }
            let mut indices = Vec::with_capacity(3 * triangles);
            for _ in 0..3 * triangles {
                let index = read_u16(&mut bytes)?;
                if usize::from(index) >= vertex_count {
                    return None;
                }
                indices.push(u32::from(index));
            }
            let area: f32 = indices
                .chunks_exact(3)
                .map(|triangle| {
                    let a = vertices[triangle[0] as usize].0;
                    let b = vertices[triangle[1] as usize].0;
                    let c = vertices[triangle[2] as usize].0;
                    (b - a).cross(c - a).length() * 0.5
                })
                .sum();
            surface_areas.push(area);
            let center = center / vertex_count as f32;
            frames.push(MeshFrame {
                vertices,
                indices: Arc::new(indices),
                anchor: Vec3::new(center.x, center.y, lo.z),
            });
        }
        // The terminal Alembic samples lose nearly all of their surface.
        // For a longer game flight, carry the last substantial baked shape
        // through the collision tick that deposits the ground blood.
        let peak_area = surface_areas.iter().copied().fold(0.0, f32::max);
        let last_visible_frame = surface_areas
            .iter()
            .rposition(|area| *area >= peak_area * 0.25)?;
        bytes.is_empty().then_some(Self {
            frames,
            fps_numerator,
            fps_denominator,
            last_visible_frame,
        })
    }

    fn sample(&self, start_ms: i32, now_ms: i32) -> Option<usize> {
        let elapsed = u64::try_from(i64::from(now_ms) - i64::from(start_ms)).ok()?;
        let frame = (elapsed * u64::from(self.fps_numerator)
            / (1000 * u64::from(self.fps_denominator))) as usize;
        Some(frame.min(self.last_visible_frame))
    }
}

struct Spurt {
    id: NonZeroU32,
    clip: Arc<Clip>,
    previous_position: Vec3,
    position: Vec3,
    rotation: Quat,
    scale: f32,
    start_ms: i32,
    impact: Option<(i32, Vec3)>,
    prepared: Option<(usize, Arc<Vec<DynamicModelVertex>>)>,
}

impl Spurt {
    fn alive(&self, now_ms: i32) -> bool {
        // The leading physics drop owns the airborne lifetime.
        self.impact
            .is_none_or(|(time, _)| now_ms.saturating_sub(time) < IMPACT_HOLD_MS)
    }

    fn draw(&mut self, frame: usize, phase: f32) -> FxDraw {
        let mesh = &self.clip.frames[frame];
        let (_, vertices) = self.prepared.get_or_insert_with(|| {
            // Rebase each baked shape onto the simulated droplet's travel.
            // At contact the receiver plane supports the lowest part of the mesh.
            let mut anchor = mesh.anchor;
            if let Some((_, normal)) = self.impact {
                let axis = self.rotation.inverse() * normal;
                let bottom = mesh
                    .vertices
                    .iter()
                    .map(|(p, _)| p.dot(axis))
                    .fold(f32::INFINITY, f32::min);
                anchor += axis * (bottom - anchor.dot(axis));
            }
            let [r, g, b] = crate::ninja_blood::blood_pigment(1.0);
            let vertices = mesh
                .vertices
                .iter()
                .map(|&(position, normal)| DynamicModelVertex {
                    position: crate::scene::render_position(
                        (self.rotation * ((position - anchor) * self.scale)).to_array(),
                    ),
                    normal: crate::scene::render_position((self.rotation * normal).to_array()),
                    uv: [0.0; 2],
                    color: [r, g, b, 1.0],
                    depth_hack: 0.0,
                    water_wetness: 0.0,
                    foliage_wind: 0.0,
                })
                .collect();
            (frame, Arc::new(vertices))
        });
        // Interpolate only between completed sweeps. Landing snaps to the
        // contact point used by the density seed on that same simulation tick.
        let origin = if let Some((_, normal)) = self.impact {
            self.position + normal * 0.15
        } else {
            self.previous_position.lerp(self.position, phase)
        };
        FxDraw::BakedLitMesh {
            vertices: Arc::clone(vertices),
            indices: Arc::clone(&mesh.indices),
            lighting_origin: origin.to_array(),
        }
    }
}

#[derive(Default)]
pub(crate) struct SpurtEffects {
    clip: Option<Arc<Clip>>,
    active: Vec<Spurt>,
    next_id: u32,
}

impl SpurtEffects {
    pub(crate) fn load(&mut self, base: &Path, game: Option<&Path>) -> Result<(), String> {
        self.clip = None;
        let asset = AssetSearchPath::open_game(base, game)
            .and_then(|mut assets| assets.read(MESH_ASSET, MAX_MESH_BYTES))
            .map_err(|error| format!("could not read {MESH_ASSET}: {error}"))?
            .ok_or_else(|| {
                format!("missing {MESH_ASSET}. Install the local spurt PK3 and run fs_refresh")
            })?;
        let clip = Clip::decode(&asset.bytes).ok_or_else(|| {
            format!("invalid {MESH_ASSET}. Rebuild the PK3 with prepare-blood-spurt.py")
        })?;
        self.clip = Some(Arc::new(clip));
        Ok(())
    }

    pub(crate) fn clear(&mut self) {
        self.active.clear();
    }

    pub(crate) fn spawn(
        &mut self,
        origin: Vec3,
        direction: Vec3,
        time_ms: i32,
        size_scale: f32,
    ) -> Option<NonZeroU32> {
        if !origin.is_finite() {
            return None;
        }
        self.active.retain(|spurt| spurt.alive(time_ms));
        if self.active.len() >= MAX_SPURTS {
            return None;
        }
        let clip = Arc::clone(self.clip.as_ref()?);
        // The hit spreads around its origin and falls along source -Z. Use the
        // contact heading for yaw while keeping the baked fall aligned to gravity.
        let yaw = direction.y.atan2(direction.x);
        let rotation = if yaw.is_finite() {
            Quat::from_rotation_z(yaw)
        } else {
            Quat::IDENTITY
        };
        self.next_id = self.next_id.wrapping_add(1).max(1);
        let id = NonZeroU32::new(self.next_id)?;
        self.active.push(Spurt {
            id,
            clip,
            previous_position: origin,
            position: origin,
            rotation,
            scale: MAP_UNITS_PER_SOURCE_UNIT * size_scale,
            start_ms: time_ms,
            impact: None,
            prepared: None,
        });
        Some(id)
    }

    pub(crate) fn follows(&self, id: NonZeroU32) -> bool {
        self.active.iter().any(|spurt| spurt.id == id)
    }

    pub(crate) fn move_to(&mut self, id: NonZeroU32, position: Vec3) {
        if let Some(spurt) = self.active.iter_mut().find(|spurt| spurt.id == id) {
            spurt.previous_position = spurt.position;
            spurt.position = position;
        }
    }

    pub(crate) fn finish(&mut self, id: NonZeroU32, impact: Option<(Vec3, Vec3)>, time_ms: i32) {
        let Some((position, normal)) = impact else {
            self.active.retain(|spurt| spurt.id != id);
            return;
        };
        if let Some(spurt) = self.active.iter_mut().find(|spurt| spurt.id == id) {
            spurt.position = position;
            spurt.impact = Some((time_ms, normal));
            spurt.prepared = None;
        }
    }

    pub(crate) fn append(&mut self, out: &mut FxFrame, time_ms: i32, phase: f32) {
        if self.active.is_empty() {
            return;
        }
        self.active.retain_mut(|spurt| {
            if !spurt.alive(time_ms) {
                return false;
            }
            let Some(frame) = spurt.clip.sample(spurt.start_ms, time_ms) else {
                return false;
            };
            if spurt
                .prepared
                .as_ref()
                .is_some_and(|(previous, _)| *previous != frame)
            {
                spurt.prepared = None;
            }
            out.draws.push(spurt.draw(frame, phase));
            true
        });
    }
}

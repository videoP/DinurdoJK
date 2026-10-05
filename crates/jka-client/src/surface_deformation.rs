//! Client-side authored-surface deformation inputs and the persistent Snowflow state field.
//!
//! 2D mode's stock JKA footprint marks are projected decals made by the footstep
//! system. 3D mode layers those same marks over
//! Snowflow's GPU state model: R=depression, G=displaced mass/berm, B=compression,
//! A=ice. Contacts are only transient brush inputs; history lives in the toroidal
//! RGBA16F field.

use crate::scene::render_position;
use crate::ui::FootprintMode;
use bytemuck::{Pod, Zeroable};
use glam::Vec2;
use jka_assets::bsp::{MATERIAL_MASK, MATERIAL_SNOW};
use jka_movement::{PlayerView, TraceQuery, TraceWorld, ENTITY_NONE};
use std::time::Instant;
use wgpu::util::DeviceExt;

const CONTENTS_SOLID: i32 = 0x0000_0001;
const CONTENTS_TERRAIN: i32 = 0x0000_1000;
const FOOT_LATERAL_OFFSET: f32 = 5.5;
const TRACE_ABOVE_FOOT: f32 = 12.0;
const TRACE_BELOW_FOOT: f32 = 24.0;

pub const MAX_VISUAL_STAMPS: usize = 32;
pub const SNOW_SHELL_TESSELLATION: f32 = 4.0;
pub const SNOW_SHELL_CHUNK_SIZE: f32 = 96.0;
pub const SNOW_SHELL_DRAW_RADIUS: f32 = 256.0;

// Snowflow source units are metres. JKA/Quake world units are inch-like; this
// preserves the source's physical brush proportions without inventing new curves.
const UNITS_PER_METRE: f32 = 39.370_08;
const METRES_PER_UNIT: f32 = 1.0 / UNITS_PER_METRE;
const SNOWFLOW_FIELD_RES: u32 = 2048;
const SNOWFLOW_FIELD_SIZE: f32 = 80.0 * UNITS_PER_METRE;
const SNOWFLOW_MAX_BRUSHES: usize = 96;
const SNOWFLOW_BRUSH_ROWS: u32 = 3;
const SNOWFLOW_RELAX_STEP: f32 = 0.4;
const SNOWFLOW_MAX_DEPTH: f32 = 0.55 * UNITS_PER_METRE;
const SNOWFLOW_BERM_SCALE: f32 = 1.5;
const SNOWFLOW_MAX_BERM: f32 = 0.34 * SNOWFLOW_BERM_SCALE * UNITS_PER_METRE;
const SNOWFLOW_WIND_ANGLE: f32 = 42.0_f32.to_radians();

fn footprint_log(level: u8, args: std::fmt::Arguments<'_>) {
    if crate::logging::developer_enabled(level) {
        crate::logging::write_line(crate::logging::Level::Info, args);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FootSide {
    Left,
    Right,
}

impl FootSide {
    fn opposite(self) -> Self {
        match self {
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }

    fn lateral_sign(self) -> f32 {
        match self {
            Self::Left => -1.0,
            Self::Right => 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceDeformationKind {
    Foot,
    Walk,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceDeformationStamp {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub forward: [f32; 3],
    pub material: u8,
    pub foot: FootSide,
    pub kind: SurfaceDeformationKind,
    /// Horizontal travel represented by this contact, in JKA world units.
    pub travel: f32,
    /// Horizontal speed at the contact, in JKA world units / second.
    pub speed: f32,
}

pub struct LocalFootContactShim {
    last_origin: [f32; 3],
    distance_since_step: f32,
    next_foot: FootSide,
    pending: Vec<SurfaceDeformationStamp>,
    was_grounded: bool,
}

impl LocalFootContactShim {
    pub fn new(origin: [f32; 3]) -> Self {
        footprint_log(2, format_args!(
            "[footprints shim] initialized grounded-distance source at {:.1},{:.1},{:.1}",
            origin[0], origin[1], origin[2]
        ));
        Self {
            last_origin: origin,
            distance_since_step: 0.0,
            next_foot: FootSide::Left,
            pending: Vec::new(),
            was_grounded: false,
        }
    }

    pub fn reset(&mut self, origin: [f32; 3]) {
        self.last_origin = origin;
        self.distance_since_step = 0.0;
        self.next_foot = FootSide::Left;
        self.pending.clear();
        self.was_grounded = false;
    }

    pub fn observe_pmove<W: TraceWorld>(&mut self, view: PlayerView, world: &mut W) {
        const STRIDE_DISTANCE: f32 = 48.0;
        const TELEPORT_DISTANCE: f32 = 192.0;
        const MIN_MOVING_SPEED: f32 = 8.0;

        let dx = view.origin[0] - self.last_origin[0];
        let dy = view.origin[1] - self.last_origin[1];
        let horizontal_delta = (dx * dx + dy * dy).sqrt();
        self.last_origin = view.origin;

        if !view.grounded() {
            self.was_grounded = false;
            self.distance_since_step = 0.0;
            return;
        }
        if !self.was_grounded {
            self.was_grounded = true;
            self.distance_since_step = 0.0;
            return;
        }
        if !horizontal_delta.is_finite() || horizontal_delta > TELEPORT_DISTANCE {
            self.distance_since_step = 0.0;
            return;
        }

        let horizontal_speed =
            (view.velocity[0] * view.velocity[0] + view.velocity[1] * view.velocity[1]).sqrt();
        if horizontal_speed < MIN_MOVING_SPEED || horizontal_delta <= 0.001 {
            return;
        }

        // Snowflow _walk(): shallow distance-scaled continuous scuff. This is
        // intentionally separate from touchdown prints so boots remain legible.
        if let Some(stamp) = trace_walk(view, world, horizontal_delta, horizontal_speed) {
            self.pending.push(stamp);
        }

        self.distance_since_step += horizontal_delta;
        while self.distance_since_step >= STRIDE_DISTANCE {
            self.distance_since_step -= STRIDE_DISTANCE;
            let foot = self.next_foot;
            self.next_foot = self.next_foot.opposite();
            if let Some(stamp) = trace_foot(view, world, foot, horizontal_speed) {
                self.pending.push(stamp);
            }
        }
    }

    pub fn take_pending(&mut self) -> Vec<SurfaceDeformationStamp> {
        std::mem::take(&mut self.pending)
    }
}

fn trace_surface<W: TraceWorld>(
    view: PlayerView,
    world: &mut W,
    lateral: f32,
) -> Option<([f32; 3], [f32; 3], [f32; 3], u8)> {
    let yaw = view.view_angles[1].to_radians();
    let right = [yaw.sin(), -yaw.cos(), 0.0];
    let forward_jka = [yaw.cos(), yaw.sin(), 0.0];
    let foot_z = view.origin[2] + view.mins[2];
    let start = [
        view.origin[0] + right[0] * lateral,
        view.origin[1] + right[1] * lateral,
        foot_z + TRACE_ABOVE_FOOT,
    ];
    let end = [start[0], start[1], foot_z - TRACE_BELOW_FOOT];
    let hit = world.trace(TraceQuery {
        start,
        mins: [0.0; 3],
        maxs: [0.0; 3],
        end,
        pass_entity: ENTITY_NONE,
        mask: CONTENTS_SOLID | CONTENTS_TERRAIN,
    });
    if hit.fraction >= 1.0 || hit.start_solid != 0 || hit.all_solid != 0 {
        return None;
    }
    let material = (hit.surface_flags as u32) & MATERIAL_MASK;
    if material != MATERIAL_SNOW {
        return None;
    }
    Some((
        render_position(hit.end),
        render_position(hit.normal),
        render_position(forward_jka),
        material as u8,
    ))
}

fn trace_foot<W: TraceWorld>(
    view: PlayerView,
    world: &mut W,
    foot: FootSide,
    speed: f32,
) -> Option<SurfaceDeformationStamp> {
    let (position, normal, forward, material) =
        trace_surface(view, world, FOOT_LATERAL_OFFSET * foot.lateral_sign())?;
    Some(SurfaceDeformationStamp {
        position,
        normal,
        forward,
        material,
        foot,
        kind: SurfaceDeformationKind::Foot,
        travel: 0.0,
        speed,
    })
}

fn trace_walk<W: TraceWorld>(
    view: PlayerView,
    world: &mut W,
    travel: f32,
    speed: f32,
) -> Option<SurfaceDeformationStamp> {
    let (position, normal, forward, material) = trace_surface(view, world, 0.0)?;
    Some(SurfaceDeformationStamp {
        position,
        normal,
        forward,
        material,
        foot: FootSide::Left,
        kind: SurfaceDeformationKind::Walk,
        travel,
        speed,
    })
}

pub fn trace_stamp(stamp: SurfaceDeformationStamp) {
    if stamp.kind == SurfaceDeformationKind::Foot {
        footprint_log(3, format_args!(
            "[footprints stamp] {:?} material={} pos={:.2},{:.2},{:.2}",
            stamp.foot, stamp.material, stamp.position[0], stamp.position[1], stamp.position[2],
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jka_movement::{TraceResult, ENTITY_WORLD};

    struct SnowWorld;
    impl TraceWorld for SnowWorld {
        fn trace(&mut self, query: TraceQuery) -> TraceResult {
            let mut result = TraceResult::clear(query.end);
            result.fraction = 0.5;
            result.end = [query.end[0], query.end[1], 0.0];
            result.normal = [0.0, 0.0, 1.0];
            result.surface_flags = MATERIAL_SNOW as i32;
            result.entity = ENTITY_WORLD;
            result
        }
        fn point_contents(&mut self, _point: [f32; 3], _pass_entity: i32) -> i32 { 0 }
    }

    fn moving_view(x: f32) -> PlayerView {
        PlayerView {
            origin: [x, 200.0, 24.0],
            velocity: [200.0, 0.0, 0.0],
            mins: [-15.0, -15.0, -24.0],
            ground_entity: ENTITY_WORLD,
            ..Default::default()
        }
    }

    #[test]
    fn local_shim_emits_walk_and_alternating_foot_contacts() {
        let mut shim = LocalFootContactShim::new([0.0, 200.0, 24.0]);
        let mut world = SnowWorld;
        shim.observe_pmove(moving_view(1.0), &mut world);
        assert!(shim.take_pending().is_empty());
        shim.observe_pmove(moving_view(50.0), &mut world);
        let first = shim.take_pending();
        assert!(first.iter().any(|s| s.kind == SurfaceDeformationKind::Walk));
        assert!(first.iter().any(|s| s.kind == SurfaceDeformationKind::Foot && s.foot == FootSide::Left));
        shim.observe_pmove(moving_view(99.0), &mut world);
        let second = shim.take_pending();
        assert!(second.iter().any(|s| s.kind == SurfaceDeformationKind::Foot && s.foot == FootSide::Right));
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct LegacyGpuStamp {
    position_half_width: [f32; 4],
    forward_half_length: [f32; 4],
    normal_depth: [f32; 4],
    meta: [u32; 4],
}

impl From<SurfaceDeformationStamp> for LegacyGpuStamp {
    fn from(stamp: SurfaceDeformationStamp) -> Self {
        Self {
            position_half_width: [stamp.position[0], stamp.position[1], stamp.position[2], 6.0],
            forward_half_length: [stamp.forward[0], stamp.forward[1], stamp.forward[2], 10.5],
            normal_depth: [stamp.normal[0], stamp.normal[1], stamp.normal[2], 8.0],
            meta: [u32::from(matches!(stamp.foot, FootSide::Right)), 0, 0, 0],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SurfaceDeformationUniform {
    header: [u32; 4],
    snowflow_header: [u32; 4],
    snowflow_params: [f32; 4], // center.x, center.z, size, texel
    snowflow_params2: [f32; 4], // metres/unit, geometry spacing, draw radius, reserved
    legacy_bounds: [f32; 4],
    stamps: [LegacyGpuStamp; MAX_VISUAL_STAMPS],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SnowflowSimParams {
    window0: [f32; 4], // center.xy, prevCenter.xy
    window1: [f32; 4], // size, res, dt, brushCount
    state0: [f32; 4],  // refillRate, maxDepth, maxBerm, windAngle
}

#[derive(Clone, Copy)]
struct SnowflowBrush {
    x: f32,
    z: f32,
    radius: f32,
    depth: f32,
    berm: f32,
    compression: f32,
    ice: f32,
    yaw: f32,
    elongation: f32,
    edge: f32,
}

impl SnowflowBrush {
    fn from_stamp(stamp: SurfaceDeformationStamp) -> Option<Self> {
        if stamp.material as u32 != MATERIAL_SNOW {
            return None;
        }
        let forward = Vec2::new(stamp.forward[0], stamp.forward[2]).normalize_or_zero();
        let yaw = if forward.length_squared() > 0.0 { forward.y.atan2(forward.x) } else { 0.0 };
        match stamp.kind {
            SurfaceDeformationKind::Foot => {
                let speed_m = stamp.speed * METRES_PER_UNIT;
                let impact = (0.35 + speed_m / 5.4).min(1.3);
                Some(Self {
                    x: stamp.position[0], z: stamp.position[2],
                    radius: 0.10 * UNITS_PER_METRE,
                    depth: (0.17 + 0.14 * impact) * UNITS_PER_METRE,
                    berm: (0.10 + 0.08 * impact) * SNOWFLOW_BERM_SCALE * UNITS_PER_METRE,
                    compression: 0.9, ice: 0.0, yaw, elongation: 1.7, edge: 1.0,
                })
            }
            SurfaceDeformationKind::Walk => {
                let moved_m = stamp.travel * METRES_PER_UNIT;
                let k = moved_m.min(0.35);
                Some(Self {
                    x: stamp.position[0], z: stamp.position[2],
                    radius: 0.22 * UNITS_PER_METRE,
                    depth: 0.20 * k * UNITS_PER_METRE,
                    berm: 0.22 * k * SNOWFLOW_BERM_SCALE * UNITS_PER_METRE,
                    compression: 0.8 * k, ice: 0.0, yaw, elongation: 1.5, edge: 0.85,
                })
            }
        }
    }
}

pub struct SurfaceDeformationGpu {
    buffer: wgpu::Buffer,
    legacy_stamps: Vec<LegacyGpuStamp>,
    mode: FootprintMode,
    legacy_mark_mask: u32,
    legacy_mark_blends: u32,

    _field_textures: [wgpu::Texture; 2],
    field_views: [wgpu::TextureView; 2],
    field_sampler: wgpu::Sampler,
    brush_texture: wgpu::Texture,
    sim_params_buffer: wgpu::Buffer,
    sim_bind_groups: [wgpu::BindGroup; 2],
    sim_pipeline: wgpu::RenderPipeline,
    pending_brushes: Vec<SnowflowBrush>,
    active_field: usize,
    field_valid: bool,
    center: Vec2,
    prev_center: Vec2,
    focus: Vec2,
    relax_owed: f32,
    last_update: Instant,
}

impl SurfaceDeformationGpu {
    pub fn new(device: &wgpu::Device) -> Self {
        let initial = SurfaceDeformationUniform {
            header: [0; 4], snowflow_header: [0; 4],
            snowflow_params: [0.0, 0.0, SNOWFLOW_FIELD_SIZE, SNOWFLOW_FIELD_SIZE / SNOWFLOW_FIELD_RES as f32],
            snowflow_params2: [METRES_PER_UNIT, SNOW_SHELL_TESSELLATION, SNOW_SHELL_DRAW_RADIUS, 0.0],
            legacy_bounds: [0.0; 4],
            stamps: [LegacyGpuStamp::zeroed(); MAX_VISUAL_STAMPS],
        };
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("surface deformation uniform"),
            contents: bytemuck::bytes_of(&initial),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let field_desc = wgpu::TextureDescriptor {
            label: Some("Snowflow persistent deformation field"),
            size: wgpu::Extent3d { width: SNOWFLOW_FIELD_RES, height: SNOWFLOW_FIELD_RES, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };
        let field_textures = [device.create_texture(&field_desc), device.create_texture(&field_desc)];
        let field_views = [
            field_textures[0].create_view(&wgpu::TextureViewDescriptor::default()),
            field_textures[1].create_view(&wgpu::TextureViewDescriptor::default()),
        ];
        let field_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Snowflow deformation bilinear wrap sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let brush_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Snowflow brush data texture"),
            size: wgpu::Extent3d { width: SNOWFLOW_MAX_BRUSHES as u32, height: SNOWFLOW_BRUSH_ROWS, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let brush_view = brush_texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sim_params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Snowflow simulation params"),
            contents: bytemuck::bytes_of(&SnowflowSimParams::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let sim_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Snowflow simulation layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let sim_bind_groups = [0usize, 1usize].map(|prev| device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Snowflow simulation bind group"), layout: &sim_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&field_views[prev]) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&field_sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&brush_view) },
                wgpu::BindGroupEntry { binding: 3, resource: sim_params_buffer.as_entire_binding() },
            ],
        }));
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Snowflow deformation simulation"),
            source: wgpu::ShaderSource::Wgsl(include_str!("snowflow_deformation_sim.wgsl").into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Snowflow simulation pipeline layout"), bind_group_layouts: &[Some(&sim_layout)], immediate_size: 0,
        });
        let sim_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Snowflow deformation simulation pipeline"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
            primitive: wgpu::PrimitiveState::default(), depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader, entry_point: Some("fs_main"), compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba16Float, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None, cache: None,
        });

        Self {
            buffer, legacy_stamps: Vec::with_capacity(MAX_VISUAL_STAMPS), mode: FootprintMode::ThreeD,
            legacy_mark_mask: 0, legacy_mark_blends: 0,
            _field_textures: field_textures, field_views, field_sampler, brush_texture, sim_params_buffer,
            sim_bind_groups, sim_pipeline, pending_brushes: Vec::with_capacity(SNOWFLOW_MAX_BRUSHES),
            active_field: 0, field_valid: false, center: Vec2::ZERO, prev_center: Vec2::splat(1.0e6),
            focus: Vec2::ZERO, relax_owed: 0.0, last_update: Instant::now(),
        }
    }

    pub fn mode(&self) -> FootprintMode { self.mode }

    pub fn set_mode(&mut self, queue: &wgpu::Queue, mode: FootprintMode) {
        self.mode = mode;
        self.upload_world_uniform(queue);
    }

    pub fn set_legacy_mark_info(&mut self, queue: &wgpu::Queue, mask: u32, blends: [u8; 2]) {
        self.legacy_mark_mask = mask;
        self.legacy_mark_blends = u32::from(blends[0]) | (u32::from(blends[1]) << 4);
        self.upload_world_uniform(queue);
    }

    pub fn buffer(&self) -> &wgpu::Buffer { &self.buffer }
    pub fn field_views(&self) -> [&wgpu::TextureView; 2] { [&self.field_views[0], &self.field_views[1]] }
    pub fn field_sampler(&self) -> &wgpu::Sampler { &self.field_sampler }
    pub fn field_center(&self) -> Vec2 { self.center }

    pub fn clear(&mut self, queue: &wgpu::Queue) {
        self.legacy_stamps.clear();
        self.pending_brushes.clear();
        self.active_field = 0;
        self.field_valid = false;
        self.center = Vec2::ZERO;
        self.prev_center = Vec2::splat(1.0e6);
        self.focus = Vec2::ZERO;
        self.relax_owed = 0.0;
        self.last_update = Instant::now();
        self.upload_world_uniform(queue);
    }

    pub fn push(&mut self, queue: &wgpu::Queue, stamp: SurfaceDeformationStamp) {
        if stamp.material as u32 != MATERIAL_SNOW { return; }
        self.focus = Vec2::new(stamp.position[0], stamp.position[2]);
        // 2D mode's prints are projected decals from the footstep system
        // (`WeaponFx::footstep`); the shader stamp only accompanies the 3D dent.
        if stamp.kind == SurfaceDeformationKind::Foot && self.mode == FootprintMode::ThreeD {
            let legacy: LegacyGpuStamp = stamp.into();
            if self.legacy_stamps.len() == MAX_VISUAL_STAMPS {
                self.legacy_stamps.rotate_left(1);
                *self.legacy_stamps.last_mut().expect("legacy stamp ring") = legacy;
            } else {
                self.legacy_stamps.push(legacy);
            }
        }
        if self.mode == FootprintMode::ThreeD && self.pending_brushes.len() < SNOWFLOW_MAX_BRUSHES {
            if let Some(brush) = SnowflowBrush::from_stamp(stamp) {
                self.pending_brushes.push(brush);
            }
        }
        self.upload_world_uniform(queue);
    }

    /// Advances Snowflow's persistent field only when it can change: new brush
    /// input, a texel-snapped window move, or a banked 0.4 s relaxation step.
    /// This preserves Snowflow's math while avoiding a 2048² pass at 2000 render FPS.
    pub fn prepare_frame(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let now = Instant::now();
        self.relax_owed += now.duration_since(self.last_update).as_secs_f32().min(0.25);
        self.last_update = now;
        if self.mode != FootprintMode::ThreeD { return; }

        let texel = SNOWFLOW_FIELD_SIZE / SNOWFLOW_FIELD_RES as f32;
        let snapped = Vec2::new((self.focus.x / texel).round() * texel, (self.focus.y / texel).round() * texel);
        let center_changed = !self.field_valid || snapped != self.center;
        let relax_due = self.relax_owed >= SNOWFLOW_RELAX_STEP;
        if self.pending_brushes.is_empty() && !center_changed && !relax_due { return; }

        let count = self.pending_brushes.len().min(SNOWFLOW_MAX_BRUSHES);
        let brushes: Vec<SnowflowBrush> = self.pending_brushes.drain(..count).collect();
        let packed = pack_brushes(&brushes);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &self.brush_texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            bytemuck::cast_slice(&packed),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some((SNOWFLOW_MAX_BRUSHES * 4 * std::mem::size_of::<f32>()) as u32), rows_per_image: Some(SNOWFLOW_BRUSH_ROWS) },
            wgpu::Extent3d { width: SNOWFLOW_MAX_BRUSHES as u32, height: SNOWFLOW_BRUSH_ROWS, depth_or_array_layers: 1 },
        );
        let relax_dt = if relax_due { let dt = self.relax_owed; self.relax_owed = 0.0; dt } else { 0.0 };
        let previous_center = if self.field_valid { self.center } else { snapped + Vec2::splat(1.0e6) };
        let params = SnowflowSimParams {
            window0: [snapped.x, snapped.y, previous_center.x, previous_center.y],
            window1: [SNOWFLOW_FIELD_SIZE, SNOWFLOW_FIELD_RES as f32, relax_dt, brushes.len() as f32],
            state0: [1.0, SNOWFLOW_MAX_DEPTH, SNOWFLOW_MAX_BERM, SNOWFLOW_WIND_ANGLE],
        };
        queue.write_buffer(&self.sim_params_buffer, 0, bytemuck::bytes_of(&params));

        let dst = 1 - self.active_field;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Snowflow deformation update") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Snowflow persistent deformation pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.field_views[dst], resolve_target: None, depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            pass.set_pipeline(&self.sim_pipeline);
            pass.set_bind_group(0, &self.sim_bind_groups[self.active_field], &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);
        self.prev_center = previous_center;
        self.center = snapped;
        self.active_field = dst;
        self.field_valid = true;
        self.upload_world_uniform(queue);
    }

    fn legacy_bounds(&self) -> [f32; 4] {
        let Some(first) = self.legacy_stamps.first() else { return [0.0; 4]; };
        let mut min_x = first.position_half_width[0];
        let mut min_z = first.position_half_width[2];
        let mut max_x = min_x;
        let mut max_z = min_z;
        for stamp in &self.legacy_stamps[1..] {
            min_x = min_x.min(stamp.position_half_width[0]);
            min_z = min_z.min(stamp.position_half_width[2]);
            max_x = max_x.max(stamp.position_half_width[0]);
            max_z = max_z.max(stamp.position_half_width[2]);
        }
        [min_x - 14.0, min_z - 14.0, max_x + 14.0, max_z + 14.0]
    }

    fn upload_world_uniform(&self, queue: &wgpu::Queue) {
        let mut stamps = [LegacyGpuStamp::zeroed(); MAX_VISUAL_STAMPS];
        stamps[..self.legacy_stamps.len()].copy_from_slice(&self.legacy_stamps);
        let legacy_bounds = self.legacy_bounds();
        let uniform = SurfaceDeformationUniform {
            header: [self.legacy_stamps.len() as u32, self.mode.shader_value(), self.legacy_mark_mask, self.legacy_mark_blends],
            snowflow_header: [self.active_field as u32, u32::from(self.field_valid), 0, 0],
            snowflow_params: [self.center.x, self.center.y, SNOWFLOW_FIELD_SIZE, SNOWFLOW_FIELD_SIZE / SNOWFLOW_FIELD_RES as f32],
            snowflow_params2: [METRES_PER_UNIT, SNOW_SHELL_TESSELLATION, SNOW_SHELL_DRAW_RADIUS, 0.0],
            legacy_bounds,
            stamps,
        };
        queue.write_buffer(&self.buffer, 0, bytemuck::bytes_of(&uniform));
    }
}

fn pack_brushes(brushes: &[SnowflowBrush]) -> Vec<f32> {
    let mut data = vec![0.0_f32; SNOWFLOW_MAX_BRUSHES * SNOWFLOW_BRUSH_ROWS as usize * 4];
    let stride = SNOWFLOW_MAX_BRUSHES * 4;
    for (i, brush) in brushes.iter().enumerate() {
        let a = i * 4;
        let seed = ((brush.x * METRES_PER_UNIT) * 0.37 + (brush.z * METRES_PER_UNIT) * 0.71) % 100.0;
        data[a..a + 4].copy_from_slice(&[brush.x, brush.z, brush.radius, brush.elongation]);
        data[stride + a..stride + a + 4].copy_from_slice(&[brush.yaw.cos(), brush.yaw.sin(), brush.depth, brush.berm]);
        data[stride * 2 + a..stride * 2 + a + 4].copy_from_slice(&[brush.compression, brush.ice, brush.edge, seed]);
    }
    data
}

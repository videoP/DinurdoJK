//! Reusable five-channel orthographic bed capture, followed by the reference
//! stages 5/6 contour reconstruction. Callers explicitly classify terrain,
//! undilated obstacles and dilated obstacles; BSP classification is separate.

use jka_water::river::boundary::WaterInfo;
use jka_water::river::capture::{Camera, Channel, Settings, CHANNELS, PREPARE_SHADER};
use jka_water::river::CENTIMETRES_PER_MAP_UNIT;
use wgpu::util::DeviceExt;

pub const RASTER_SHADER: &str = include_str!("river_capture.wgsl");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeshClass {
    Landscape,
    Actors,
    DilatedActors,
}

/// Vertices are Z-up centimetres relative to the simulation origin. Winding is
/// authored in XY as viewed from above; the capture handles underside reversal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sides {
    TwoSided,
    CounterClockwise,
    Clockwise,
}

pub struct Mesh<'a> {
    pub class: MeshClass,
    pub positions: &'a [[f32; 3]],
    pub indices: &'a [u32],
    pub sides: Sides,
}

/// Explicit map-to-reference classification. A BSP does not carry Unreal's
/// Landscape/Actor types. Match authored shader names, not guessed terrain
/// heights, normals or all world geometry. Unselected materials are excluded.
pub struct Selection<'a> {
    pub shader: &'a str,
    pub class: MeshClass,
    pub sides: Sides,
}

/// The source-level riverbed rule shared by the dense patch and sparse
/// tiles: overhead brushes whose solid starts above `water_level` are
/// dropped, grounded solids are kept whole and terrain without brush
/// thickness is clipped to the level. Returns a convex polygon (3 or 4
/// vertices, map units); None when nothing remains.
pub fn clip_collision(
    t: &[[f32; 3]; 3],
    solid_bottom_z: Option<f32>,
    water_level: Option<f32>,
) -> Option<([[f32; 3]; 4], usize)> {
    let mut polygon = [[0.0f32; 3]; 4];
    let count = if let Some(level) = water_level {
        if solid_bottom_z.is_some_and(|bottom| bottom > level) { return None; }
        if solid_bottom_z.is_some() { polygon[..3].copy_from_slice(t); 3 }
        else {
            let mut count = 0;
            let mut previous = t[2];
            for &current in t {
                let prev_inside = previous[2] <= level;
                let curr_inside = current[2] <= level;
                if prev_inside != curr_inside {
                    let frac = (level - previous[2]) / (current[2] - previous[2]);
                    let mut p = std::array::from_fn(|a| previous[a] + frac * (current[a] - previous[a]));
                    p[2] = level; polygon[count] = p; count += 1;
                }
                if curr_inside { polygon[count] = current; count += 1; }
                previous = current;
            }
            count
        }
    } else { polygon[..3].copy_from_slice(t); 3 };
    (count >= 3).then_some((polygon, count))
}

/// A steep patch/soup face (`CollisionSurface::wall`) for the top-down
/// capture. A vertical patch projects to a line with no area, so water would
/// pass straight through it. Widen its XY footprint by `half_width` (map
/// units) on every side into a rectangle at the face's highest point; with
/// `half_width` at 0.75 cell the covered cells form an unbroken (8-connected)
/// barrier, which the 4-neighbour solver cannot cross. A face entirely above
/// the water level (arch, overhang) is dropped so water flows under it.
pub fn wall_slab(t: &[[f32; 3]; 3], half_width: f32, water_level: Option<f32>) -> Option<[[f32; 3]; 4]> {
    let bottom = t.iter().map(|p| p[2]).fold(f32::INFINITY, f32::min);
    let top = t.iter().map(|p| p[2]).fold(f32::NEG_INFINITY, f32::max);
    if water_level.is_some_and(|level| bottom > level) {
        return None;
    }
    let edge = |i: usize, j: usize| (t[j][0] - t[i][0]).hypot(t[j][1] - t[i][1]);
    let (a, b) = [(0, 1), (1, 2), (2, 0)]
        .into_iter()
        .max_by(|&(i, j), &(k, l)| edge(i, j).total_cmp(&edge(k, l)))
        .expect("three edges");
    let length = edge(a, b);
    let u = if length > 1e-4 { [(t[b][0] - t[a][0]) / length, (t[b][1] - t[a][1]) / length] } else { [1.0, 0.0] };
    let v = [-u[1], u[0]];
    let mut range = [[0.0f32, 0.0f32]; 2];
    for p in t {
        let r = [p[0] - t[a][0], p[1] - t[a][1]];
        for (axis, d) in [u, v].into_iter().enumerate() {
            let s = r[0] * d[0] + r[1] * d[1];
            range[axis] = [range[axis][0].min(s), range[axis][1].max(s)];
        }
    }
    let corner = |su: f32, sv: f32| [t[a][0] + u[0] * su + v[0] * sv, t[a][1] + u[1] * su + v[1] * sv, top];
    let [lu, hu] = [range[0][0] - half_width, range[0][1] + half_width];
    let [lv, hv] = [range[1][0] - half_width, range[1][1] + half_width];
    Some([corner(lu, lv), corner(hu, lv), corner(hu, hv), corner(lu, hv)])
}

/// One collision surface's riverbed polygon (3 or 4 vertices, map units):
/// a wall slab for steep mesh faces, `clip_collision` otherwise.
pub fn bed_polygon(
    t: &[[f32; 3]; 3],
    solid_bottom_z: Option<f32>,
    wall: bool,
    water_level: Option<f32>,
    cell_size: f32,
) -> Option<([[f32; 3]; 4], usize)> {
    if wall {
        wall_slab(t, 0.75 * cell_size, water_level).map(|p| (p, 4))
    } else {
        clip_collision(t, solid_bottom_z, water_level)
    }
}

pub struct BspCapture {
    positions: Vec<[f32; 3]>,
    groups: Vec<(MeshClass, Sides, Vec<u32>)>,
}

impl BspCapture {
    /// Use the same prepared/tessellated world mesh that the map renderer uses.
    /// Source/sink/river trigger volumes are not world rendering geometry.
    pub fn from_world(
        bsp: &jka_assets::bsp::Bsp,
        world: &jka_assets::bsp::Mesh,
        origin_map_units: [f32; 3],
        selection: &[Selection<'_>],
    ) -> Result<Self, String> {
        if origin_map_units.iter().any(|v| !v.is_finite())
            || selection.iter().enumerate().any(|(i, s)| {
                s.shader.is_empty()
                    || selection[..i]
                        .iter()
                        .any(|p| p.shader.eq_ignore_ascii_case(s.shader))
            })
        {
            return Err("invalid river capture origin or duplicate/empty shader selection".into());
        }
        let mut groups: Vec<(MeshClass, Sides, Vec<u32>)> = Vec::new();
        // Compact only selected vertices; the map mesh itself stays shared by
        // its normal owner. Source and converted vertices are not cached twice.
        let mut remap = std::collections::HashMap::<u32, u32>::new();
        let mut positions = Vec::new();
        for batch in &world.batches {
            let shader = bsp
                .shaders
                .get(batch.shader)
                .ok_or("invalid river world shader index")?;
            let Some(rule) = selection
                .iter()
                .find(|s| shader.name.eq_ignore_ascii_case(s.shader.as_bytes()))
            else {
                continue;
            };
            let indices = world
                .indices
                .get(batch.indices.clone())
                .ok_or("invalid river world index range")?;
            if !indices.len().is_multiple_of(3) {
                return Err("river world selection isn't triangular".into());
            }
            let group = match groups
                .iter()
                .position(|(class, sides, _)| *class == rule.class && *sides == rule.sides)
            {
                Some(i) => i,
                None => {
                    groups.push((rule.class, rule.sides, Vec::new()));
                    groups.len() - 1
                }
            };
            for &index in indices {
                let compact = match remap.get(&index) {
                    Some(&i) => i,
                    None => {
                        let vertex = world
                            .vertices
                            .get(index as usize)
                            .ok_or("invalid river world vertex index")?;
                        let converted = std::array::from_fn::<_, 3, _>(|axis| {
                            (vertex.position[axis] - origin_map_units[axis])
                                * CENTIMETRES_PER_MAP_UNIT
                        });
                        if converted.iter().any(|v| !v.is_finite()) {
                            return Err("nonfinite river capture world vertex".into());
                        }
                        let i = u32::try_from(positions.len())
                            .map_err(|_| "too many river capture vertices")?;
                        positions.push(converted);
                        remap.insert(index, i);
                        i
                    }
                };
                groups[group].2.push(compact);
            }
        }
        if positions.is_empty() {
            return Err("river capture shader selection contains no world triangles".into());
        }
        Ok(Self { positions, groups })
    }

    /// Generic riverbed: the map's collision geometry
    /// (`jka_assets::bsp::river::collision_triangles`) as two-sided Landscape,
    /// limited to triangles overlapping the capture rectangle (`half_extent`,
    /// map units, around `origin_map_units`). Sky is excluded upstream; any
    /// roof or overhang over the water is captured as ground (the native
    /// bridge rule applies only to Actors).
    pub fn from_collision(
        triangles: &[[[f32; 3]; 3]],
        origin_map_units: [f32; 3],
        half_extent: [f32; 2],
    ) -> Result<Self, String> {
        Self::capture_collision(triangles.iter().map(|t| (t, None, false)), origin_map_units, half_extent, None, 0.0)
    }

    /// Keep grounded solid obstacles that intersect the water, while dropping
    /// overhead brushes and clipping terrain without brush thickness. Steep
    /// patch faces become wall slabs sized for `cell_size` (map units).
    pub fn from_collision_for_source(
        surfaces: &[jka_assets::bsp::river::CollisionSurface],
        origin_map_units: [f32; 3],
        half_extent: [f32; 2],
        water_level: f32,
        cell_size: f32,
    ) -> Result<Self, String> {
        if !water_level.is_finite() || !(cell_size.is_finite() && cell_size > 0.0) {
            return Err("invalid river capture water level or cell size".into());
        }
        Self::capture_collision(
            surfaces.iter().map(|s| (&s.triangle, s.solid_bottom_z, s.wall)),
            origin_map_units,
            half_extent,
            Some(water_level),
            cell_size,
        )
    }

    fn capture_collision<'a>(
        triangles: impl IntoIterator<Item = (&'a [[f32; 3]; 3], Option<f32>, bool)>,
        origin_map_units: [f32; 3],
        half_extent: [f32; 2],
        water_level: Option<f32>,
        cell_size: f32,
    ) -> Result<Self, String> {
        if origin_map_units.iter().chain(&half_extent).any(|v| !v.is_finite())
            || half_extent.iter().any(|v| *v <= 0.0)
        {
            return Err("invalid river collision capture origin or extent".into());
        }
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for (t, solid_bottom_z, wall) in triangles {
            let Some((polygon, count)) = bed_polygon(t, solid_bottom_z, wall, water_level, cell_size) else { continue; };
            let outside = (0..2).any(|a| {
                let lo = polygon[..count].iter().map(|p| p[a]).fold(f32::INFINITY, f32::min);
                let hi = polygon[..count].iter().map(|p| p[a]).fold(f32::NEG_INFINITY, f32::max);
                hi < origin_map_units[a] - half_extent[a] || lo > origin_map_units[a] + half_extent[a]
            });
            if outside {
                continue;
            }
            for k in 2..count {
                for p in [polygon[0], polygon[k - 1], polygon[k]] {
                let converted = std::array::from_fn::<_, 3, _>(|axis| {
                    (p[axis] - origin_map_units[axis]) * CENTIMETRES_PER_MAP_UNIT
                });
                if converted.iter().any(|v| !v.is_finite()) {
                    return Err("nonfinite river collision vertex".into());
                }
                indices.push(u32::try_from(positions.len()).map_err(|_| "too many river capture vertices")?);
                positions.push(converted);
                }
            }
        }
        if positions.is_empty() {
            return Err(if water_level.is_some() { "no collision geometry in the river capture area near or below the water" } else { "no collision geometry inside the river capture area" }.into());
        }
        Ok(Self {
            positions,
            groups: vec![(MeshClass::Landscape, Sides::TwoSided, indices)],
        })
    }

    /// `r_riverCaptureMode 1`: the reachable bed (`river_reach::classify`)
    /// as Landscape and the structures above it as Actors, both two-sided, so
    /// the Actors capture records their real tops and undersides. Triangles
    /// outside the capture rectangle are dropped.
    pub fn from_classified(
        landscape: &[[[f32; 3]; 3]],
        actors: &[[[f32; 3]; 3]],
        origin_map_units: [f32; 3],
        half_extent: [f32; 2],
    ) -> Result<Self, String> {
        if origin_map_units.iter().chain(&half_extent).any(|v| !v.is_finite())
            || half_extent.iter().any(|v| *v <= 0.0)
        {
            return Err("invalid river collision capture origin or extent".into());
        }
        let mut positions = Vec::new();
        let mut groups = Vec::new();
        for (class, triangles) in [(MeshClass::Landscape, landscape), (MeshClass::Actors, actors)] {
            let mut indices = Vec::new();
            for t in triangles {
                let outside = (0..2).any(|a| {
                    let lo = t.iter().map(|p| p[a]).fold(f32::INFINITY, f32::min);
                    let hi = t.iter().map(|p| p[a]).fold(f32::NEG_INFINITY, f32::max);
                    hi < origin_map_units[a] - half_extent[a] || lo > origin_map_units[a] + half_extent[a]
                });
                if outside {
                    continue;
                }
                for p in t {
                    let converted = std::array::from_fn::<_, 3, _>(|axis| {
                        (p[axis] - origin_map_units[axis]) * CENTIMETRES_PER_MAP_UNIT
                    });
                    if converted.iter().any(|v| !v.is_finite()) {
                        return Err("nonfinite river collision vertex".into());
                    }
                    indices.push(u32::try_from(positions.len()).map_err(|_| "too many river capture vertices")?);
                    positions.push(converted);
                }
            }
            if class == MeshClass::Landscape && indices.is_empty() {
                return Err("no reachable river bed in the capture area (r_riverCaptureMode 1)".into());
            }
            groups.push((class, Sides::TwoSided, indices));
        }
        Ok(Self { positions, groups })
    }

    pub fn meshes(&self) -> Vec<Mesh<'_>> {
        self.groups
            .iter()
            .map(|(class, sides, indices)| Mesh {
                class: *class,
                positions: &self.positions,
                indices,
                sides: *sides,
            })
            .collect()
    }

    pub fn vertical_bounds(&self) -> [f32; 2] {
        self.positions
            .iter()
            .fold([f32::INFINITY, f32::NEG_INFINITY], |[lo, hi], v| {
                [lo.min(v[2]), hi.max(v[2])]
            })
    }
}

/// This retains only geometry and its first-step history. The encoder owns
/// transient contour/WaterInfo bindings until the submitted work finishes.
pub struct PreparedBed {
    pub dimensions: [u32; 2],
    pub geometry: wgpu::Buffer,
    pub previous_bottom: wgpu::Buffer,
    pub settings: Settings,
    pub water_info_hash: [u8; 32],
}

impl PreparedBed {
    pub fn geometry(&self) -> &wgpu::Buffer {
        &self.geometry
    }
    pub fn logical_bytes(&self) -> u64 {
        u64::from(self.dimensions[0]) * u64::from(self.dimensions[1]) * 20
    }
}

pub struct DepthCaptures {
    texture: wgpu::Texture,
    resolution: u32,
    width: f32,
    cameras: [Camera; CHANNELS],
    depth: wgpu::Texture,
    layout: wgpu::BindGroupLayout,
    pipelines: Vec<wgpu::RenderPipeline>,
    captured: bool,
}

impl DepthCaptures {
    pub fn new(
        device: &wgpu::Device,
        resolution: u32,
        width: f32,
        cameras: [Camera; CHANNELS],
    ) -> Result<Self, String> {
        let limits = device.limits();
        Settings {
            dimensions: [1, 1],
            world_size: [width; 2],
            capture_width: width,
            dx: width,
            bottom_depth: 0.0,
            dilation: 0.0,
            cameras,
        }
        .validate()?;
        if resolution == 0
            || resolution > limits.max_texture_dimension_2d
            || limits.max_texture_array_layers < CHANNELS as u32
        {
            return Err("river capture resolution exceeds the GPU limits".into());
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("UE river five reusable depth captures"),
            size: wgpu::Extent3d {
                width: resolution,
                height: resolution,
                depth_or_array_layers: CHANNELS as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("UE river shared capture depth attachment"),
            size: wgpu::Extent3d {
                width: resolution,
                height: resolution,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("UE river capture camera"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("UE river capture layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("UE river orthographic depth capture"),
            source: wgpu::ShaderSource::Wgsl(RASTER_SHADER.into()),
        });
        let pipelines = [None, Some(wgpu::Face::Back), Some(wgpu::Face::Front)]
            .into_iter()
            .map(|cull_mode| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("UE river scene depth"),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("capture_vertex"),
                        compilation_options: Default::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: 12,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                        })],
                    },
                    primitive: wgpu::PrimitiveState {
                        front_face: wgpu::FrontFace::Cw,
                        cull_mode,
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: Some(true),
                        depth_compare: Some(wgpu::CompareFunction::Less),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some("capture_depth"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: wgpu::TextureFormat::R32Float,
                            blend: None,
                            write_mask: wgpu::ColorWrites::RED,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            })
            .collect();
        Ok(Self {
            texture,
            resolution,
            width,
            cameras,
            depth,
            layout,
            pipelines,
            captured: false,
        })
    }

    /// No readbacks or per-cell rays. Each mesh uploads once for the capture;
    /// top/under passes reuse it. Calling this again replaces all five layers.
    pub fn encode_capture(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        meshes: &[Mesh<'_>],
    ) -> Result<(), String> {
        for mesh in meshes {
            if !mesh.indices.len().is_multiple_of(3)
                || mesh.indices.len() > u32::MAX as usize
                || std::mem::size_of_val(mesh.positions) as u64 > device.limits().max_buffer_size
                || std::mem::size_of_val(mesh.indices) as u64 > device.limits().max_buffer_size
                || mesh.positions.iter().flatten().any(|v| !v.is_finite())
                || mesh
                    .indices
                    .iter()
                    .any(|&i| i as usize >= mesh.positions.len())
            {
                return Err("invalid river capture triangle mesh".into());
            }
        }
        let mut vertex_buffers = std::collections::HashMap::new();
        let uploaded: Vec<_> = meshes
            .iter()
            .filter(|m| !m.indices.is_empty())
            .map(|mesh| {
                let vertices = vertex_buffers
                    .entry((mesh.positions.as_ptr() as usize, mesh.positions.len()))
                    .or_insert_with(|| {
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("UE river capture vertices"),
                            contents: bytemuck::cast_slice(mesh.positions),
                            usage: wgpu::BufferUsages::VERTEX,
                        })
                    })
                    .clone();
                let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("UE river capture indices"),
                    contents: bytemuck::cast_slice(mesh.indices),
                    usage: wgpu::BufferUsages::INDEX,
                });
                (mesh, vertices, indices)
            })
            .collect();
        let depth = self.depth.create_view(&Default::default());
        for channel in Channel::ALL {
            let camera = self.cameras[channel as usize];
            let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("UE river capture projection"),
                contents: bytemuck::cast_slice(&[
                    self.width,
                    camera.height,
                    if channel.top_down() { -1.0 } else { 1.0 },
                    0.0,
                    camera.near,
                    camera.far,
                    0.0,
                    0.0,
                ]),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("UE river capture projection binding"),
                layout: &self.layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                }],
            });
            let view = self.texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: channel as u32,
                array_layer_count: Some(1),
                ..Default::default()
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("UE river orthographic capture pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: camera.far as f64,
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, &group, &[]);
            let class = match channel {
                Channel::Landscape => MeshClass::Landscape,
                Channel::ActorsTop | Channel::ActorsUnder => MeshClass::Actors,
                Channel::DilatedTop | Channel::DilatedUnder => MeshClass::DilatedActors,
            };
            for (mesh, vertices, indices) in &uploaded {
                if mesh.class != class {
                    continue;
                }
                let pipeline = match (mesh.sides, channel.top_down()) {
                    (Sides::TwoSided, _) => 0,
                    (Sides::CounterClockwise, true) | (Sides::Clockwise, false) => 1,
                    _ => 2,
                };
                pass.set_pipeline(&self.pipelines[pipeline]);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.indices.len() as u32, 0, 0..1);
            }
        }
        self.captured = true;
        Ok(())
    }

    /// Reuse the captured geometry with different water levels, bottom floor
    /// and dilation. The five images themselves remain independent of a solve.
    pub fn encode_prepare(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        settings: Settings,
        info: &[WaterInfo],
    ) -> Result<PreparedBed, String> {
        let field =
            crate::river_water_info::Field::from_samples(device, settings.dimensions, info)?;
        self.encode_prepare_field(device, encoder, settings, &field)
    }

    /// Consume the very same immutable GPU WaterInfo field the solver will
    /// bind. Merge/finalize/decode, bed preparation and solve may queue together.
    pub fn encode_prepare_field(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        settings: Settings,
        info: &crate::river_water_info::Field,
    ) -> Result<PreparedBed, String> {
        settings.validate()?;
        let count = u64::from(settings.dimensions[0]) * u64::from(settings.dimensions[1]);
        let contour_bytes = count.checked_mul(20).ok_or("river contour size overflow")?;
        let limits = device.limits();
        if !self.captured
            || settings.capture_width != self.width
            || settings
                .cameras
                .iter()
                .zip(self.cameras)
                .any(|(a, b)| a.height != b.height || a.near != b.near || a.far != b.far)
            || settings.dimensions != info.dimensions
            || count * 16 != info.buffer.size()
            || contour_bytes > u64::from(limits.max_storage_buffer_binding_size)
            || contour_bytes > limits.max_buffer_size
            || settings
                .dimensions
                .iter()
                .any(|n| n.div_ceil(8) > limits.max_compute_workgroups_per_dimension)
        {
            return Err(
                "river capture/WaterInfo dimensions, camera identity or GPU limits differ".into(),
            );
        }
        let mut uniform: Vec<u8> = [
            settings.dimensions[0],
            settings.dimensions[1],
            settings.dilation_radius() as u32,
            self.resolution,
        ]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
        uniform.extend(
            [
                settings.world_size[0],
                settings.world_size[1],
                self.width,
                settings.floor(),
            ]
            .into_iter()
            .flat_map(f32::to_le_bytes),
        );
        for channel in Channel::ALL {
            uniform.extend(
                [
                    self.cameras[channel as usize].height,
                    if channel.top_down() { -1.0 } else { 1.0 },
                    0.0,
                    0.0,
                ]
                .into_iter()
                .flat_map(f32::to_le_bytes),
            );
        }
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("UE river contour preparation controls"),
            contents: &uniform,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let water_info_hash = info.identity;
        let water_info = &info.buffer;
        let buffer = |label, stride| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: count * stride,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })
        };
        let contours = buffer("UE river projected raw contours", 20);
        let geometry = buffer("UE river reconstructed geometry", 16);
        let previous_bottom = buffer("UE river initial bottom history", 4);
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("UE river contour preparation layout"),
            entries: &(0..6)
                .map(|binding| wgpu::BindGroupLayoutEntry {
                    binding,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    count: None,
                    ty: if binding == 1 {
                        wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2Array,
                            multisampled: false,
                        }
                    } else {
                        wgpu::BindingType::Buffer {
                            ty: match binding {
                                0 => wgpu::BufferBindingType::Uniform,
                                3 => wgpu::BufferBindingType::Storage { read_only: true },
                                _ => wgpu::BufferBindingType::Storage { read_only: false },
                            },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        }
                    },
                })
                .collect::<Vec<_>>(),
        });
        let view = self.texture.create_view(&Default::default());
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("UE river contour preparation bindings"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: contours.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: water_info.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: geometry.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: previous_bottom.as_entire_binding(),
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("UE river contour pipeline layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("UE river contour reconstruction"),
            source: wgpu::ShaderSource::Wgsl(PREPARE_SHADER.into()),
        });
        for entry in ["project_contours", "prepare_geometry"] {
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(entry),
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(
                settings.dimensions[0].div_ceil(8),
                settings.dimensions[1].div_ceil(8),
                1,
            );
        }
        Ok(PreparedBed {
            dimensions: settings.dimensions,
            geometry,
            previous_bottom,
            settings,
            water_info_hash,
        })
    }

    /// Payload only, excluding mesh uploads and implementation allocation padding.
    pub fn logical_bytes(&self) -> u64 {
        u64::from(self.resolution).pow(2) * (CHANNELS as u64 + 1) * 4
    }

    /// The five linear-depth layers of the latest capture (sparse tile commit).
    pub fn depth_layers(&self) -> &wgpu::Texture {
        &self.texture
    }

    pub fn resolution(&self) -> u32 {
        self.resolution
    }
}

#[cfg(test)]
mod wall_slab_tests {
    use super::*;

    #[test]
    fn vertical_patch_face_becomes_a_widened_wall_unless_overhead() {
        // A vertical face along X: no area from above.
        let face = [[0.0, 0.0, 0.0], [64.0, 0.0, 0.0], [64.0, 0.0, 100.0]];
        let slab = wall_slab(&face, 12.0, Some(50.0)).expect("grounded wall is kept");
        let xs = slab.map(|p| p[0]);
        let ys = slab.map(|p| p[1]);
        assert!(slab.iter().all(|p| p[2] == 100.0), "flat at the face's top");
        assert_eq!(xs.iter().copied().fold(f32::INFINITY, f32::min), -12.0);
        assert_eq!(xs.iter().copied().fold(f32::NEG_INFINITY, f32::max), 76.0);
        assert_eq!(ys.iter().copied().fold(f32::INFINITY, f32::min), -12.0);
        assert_eq!(ys.iter().copied().fold(f32::NEG_INFINITY, f32::max), 12.0);
        // Entirely above the water: an arch the river passes under.
        let arch = face.map(|p| [p[0], p[1], p[2] + 200.0]);
        assert!(wall_slab(&arch, 12.0, Some(50.0)).is_none());
        // Ground faces still take the clip path.
        let floor = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]];
        assert_eq!(bed_polygon(&floor, None, false, Some(50.0), 16.0).map(|(_, n)| n), Some(3));
    }
}
